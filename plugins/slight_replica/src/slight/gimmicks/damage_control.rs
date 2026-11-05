//! DamageModule writes are drained on the exact owning fighter's Main line.
use super::native_inputs::{CombatChannel, CombatOp};
use super::{
    actor_identity::Identity,
    damage_commands::{Lease, Queue, Touched},
    phase_commands::Command,
};
use smash::app::{
    lua_bind::{
        AttackModule, DamageModule, HitModule, ItemModule, KineticModule, ModelModule,
        PostureModule, StatusModule,
    },
    BattleObjectModuleAccessor, DamageNoReactionMode, HitStatus, ItemKind,
};
use smash::lib::lua_const::*;
use std::sync::atomic::{AtomicU64, Ordering};

static REVISION: AtomicU64 = AtomicU64::new(0);
static LAST_MESH: AtomicU64 = AtomicU64::new(0);
static MESH_NOTES: AtomicU64 = AtomicU64::new(0);
static QUEUE: std::sync::LazyLock<parking_lot::Mutex<Queue<Identity>>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(Queue::default()));

pub(crate) fn configuration_changed() {
    super::grab_control::configuration_changed();
    super::item_control::configuration_changed();
    // TCP/configuration paths never call engine APIs. The next owning Main
    // boundary retires old leases even when the replacement graph is empty.
    REVISION.fetch_add(1, Ordering::SeqCst);
    MESH_NOTES.store(0, Ordering::Relaxed);
}
fn check(boid: u32, fighter: &str, identity: Identity) -> Result<(), String> {
    if Identity::from_live_fighter(boid, fighter) == Some(identity) {
        Ok(())
    } else {
        Err("fighter identity changed during damage control".into())
    }
}
pub(crate) fn preflight(
    boid: u32,
    fighter: &str,
    identity: Identity,
    count: usize,
) -> Result<(), String> {
    check(boid, fighter, identity)?;
    if !(0..8).contains(&identity.entry_id) {
        return Err("invalid damage command fighter entry".into());
    }
    let queue = QUEUE.lock();
    if queue.identity(boid).is_some_and(|old| old != identity) {
        return Err("fighter command cleanup pending on owning Main line".into());
    }
    queue.preflight(boid, identity, count)
}
pub(crate) fn enqueue(
    boid: u32,
    fighter: &str,
    identity: Identity,
    command: Command,
) -> Result<(), String> {
    check(boid, fighter, identity)?;
    if !(0..8).contains(&identity.entry_id) {
        return Err("invalid damage command fighter entry".into());
    }
    if QUEUE
        .lock()
        .identity(boid)
        .is_some_and(|old| old != identity)
    {
        return Err("fighter command cleanup pending on owning Main line".into());
    }
    let module = identity.module_accessor_addr as *mut BattleObjectModuleAccessor;
    let status = unsafe { StatusModule::status_kind(module) };
    check(boid, fighter, identity)?;
    let lease = Lease {
        status,
        revision: REVISION.load(Ordering::SeqCst),
    };
    QUEUE
        .lock()
        .enqueue(boid, fighter, identity, lease, command)
}
fn restore(boid: u32, fighter: &str, identity: Identity, touched: Touched) -> Result<(), String> {
    let module = identity.module_accessor_addr as *mut BattleObjectModuleAccessor;
    if touched.gravity || touched.hidden_meshes != [0; 8] {
        // A stock can advance before Main observes the expired lease. Only
        // restore on the same live object/accessor, re-proving its NEW stock;
        // never use this exception to deliver an old stock's queued commands.
        let current =
            Identity::from_live_fighter(boid, fighter).ok_or("fighter owner unavailable")?;
        if current.fighter_kind != identity.fighter_kind
            || current.entry_id != identity.entry_id
            || current.module_accessor_addr != identity.module_accessor_addr
        {
            return Err("fighter owner was replaced".into());
        }
        check(boid, fighter, current)?;
        unsafe {
            if touched.gravity {
                KineticModule::enable_energy(module, *FIGHTER_KINETIC_ENERGY_ID_GRAVITY);
            }
            for mesh in touched.hidden_meshes.into_iter().filter(|mesh| *mesh != 0) {
                ModelModule::set_mesh_visibility(module, smash::phx::Hash40::new_raw(mesh), true);
                crate::slight::diag::note(format!("mesh object={boid} {mesh:#x} shown again"));
            }
        }
    }
    for channel in CombatChannel::ALL.into_iter().filter(|c| touched.has(*c)) {
        check(boid, fighter, identity)?;
        unsafe {
            match channel {
                CombatChannel::DamageTaken => DamageModule::set_damage_mul(module, 1.0),
                CombatChannel::KnockbackTaken => DamageModule::set_reaction_mul(module, 1.0),
                CombatChannel::DamageDealt => AttackModule::set_power_mul(module, 1.0),
                CombatChannel::KnockbackDealt => AttackModule::set_reaction_mul(module, 1.0),
                CombatChannel::Armor => {
                    DamageModule::reset_no_reaction_mode_status(module);
                }
                // Vanilla statuses end XLU/INVINCIBLE windows with HIT_STATUS_NORMAL.
                CombatChannel::Hurtbox => {
                    HitModule::set_whole(module, HitStatus(*HIT_STATUS_NORMAL), 0)
                }
                CombatChannel::RunSpeed
                | CombatChannel::AirSpeed
                | CombatChannel::Gravity
                | CombatChannel::JumpHeight => {
                    if let Some(stat) = channel.stat() {
                        super::live_parameters::set_stat(
                            boid,
                            identity.stock_generation,
                            stat,
                            1.0,
                        );
                    }
                }
            }
        }
        crate::slight::diag::note(format!(
            "combat policy object={boid} {} restored",
            channel.label()
        ));
    }
    check(boid, fighter, identity)
}
pub(crate) fn retire(boid: u32) {
    super::grab_control::retire(boid);
    super::item_control::retire(boid);
    let actor = QUEUE.lock().take(boid);
    if let Some(actor) = actor {
        // Lifecycle hooks run on the game thread. A recycled identity is
        // discarded without ever writing through its old accessor.
        let _ = restore(boid, &actor.fighter, actor.identity, actor.touched);
    }
}

pub(crate) fn flush(boid: u32) {
    // Take before entering native code, as status/damage callbacks can reenter.
    let actor = QUEUE.lock().take(boid);
    let Some(mut actor) = actor else {
        return;
    };
    let identity = actor.identity;
    if check(boid, &actor.fighter, identity).is_err() {
        let _ = restore(
            boid,
            &actor.fighter,
            identity,
            Touched {
                gravity: actor.touched.gravity,
                hidden_meshes: actor.touched.hidden_meshes,
                ..Touched::default()
            },
        );
        return;
    }
    let module = identity.module_accessor_addr as *mut BattleObjectModuleAccessor;
    let current = Lease {
        status: unsafe { StatusModule::status_kind(module) },
        revision: REVISION.load(Ordering::SeqCst),
    };
    if check(boid, &actor.fighter, identity).is_err() {
        return;
    }
    let (reset, commands) = actor.prepare(current);
    if restore(boid, &actor.fighter, identity, reset).is_err() {
        return;
    }
    for command in commands {
        if check(boid, &actor.fighter, identity).is_err() {
            return;
        }
        if REVISION.load(Ordering::SeqCst) != current.revision
            || unsafe { StatusModule::status_kind(module) } != current.status
        {
            break;
        }
        if check(boid, &actor.fighter, identity).is_err() {
            return;
        }
        unsafe {
            match command {
                Command::Movement(op) => {
                    use super::native_inputs::{Facing, MovementAction};
                    match op {
                        MovementAction::SetSpeed { x, y } => {
                            let attr = *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN;
                            let current = KineticModule::get_sum_speed3f(module, attr);
                            if check(boid, &actor.fighter, identity).is_err() {
                                return;
                            }
                            let speed = smash::phx::Vector3f {
                                x: x.unwrap_or(current.x),
                                y: y.unwrap_or(current.y),
                                z: current.z,
                            };
                            if ![speed.x, speed.y, speed.z]
                                .iter()
                                .all(|v| v.is_finite() && v.abs() <= 1000.0)
                            {
                                continue;
                            }
                            if check(boid, &actor.fighter, identity).is_err() {
                                return;
                            }
                            // Replace summed world velocity, independent of the status
                            // energy layout. Preserve omitted axes and depth; leave
                            // energy enable flags, acceleration and limits untouched.
                            KineticModule::clear_speed_all(module);
                            if check(boid, &actor.fighter, identity).is_err() {
                                return;
                            }
                            KineticModule::add_speed(module, &speed);
                        }
                        MovementAction::SetGravity { enabled } => {
                            if enabled {
                                KineticModule::enable_energy(
                                    module,
                                    *FIGHTER_KINETIC_ENERGY_ID_GRAVITY,
                                );
                            } else {
                                KineticModule::unable_energy(
                                    module,
                                    *FIGHTER_KINETIC_ENERGY_ID_GRAVITY,
                                );
                            }
                        }
                        MovementAction::MoveBy { dx, dy } => {
                            let x = PostureModule::pos_x(module) + dx;
                            let y = PostureModule::pos_y(module) + dy;
                            if ![x, y].iter().all(|v| v.is_finite() && v.abs() <= 100_000.0) {
                                continue;
                            }
                            if check(boid, &actor.fighter, identity).is_err() {
                                return;
                            }
                            PostureModule::add_pos(
                                module,
                                &smash::phx::Vector3f {
                                    x: dx,
                                    y: dy,
                                    z: 0.0,
                                },
                            );
                        }
                        MovementAction::SetFacing(facing) => {
                            let lr = match facing {
                                Facing::Left => -1.0,
                                Facing::Right => 1.0,
                                Facing::Turn => -PostureModule::lr(module),
                                Facing::Opponent => {
                                    let Some(opponent) =
                                        super::native_values::nearest_opponent(identity)
                                    else {
                                        continue;
                                    };
                                    let dx = PostureModule::pos_x(opponent)
                                        - PostureModule::pos_x(module);
                                    let lr = dx.signum();
                                    // Level with them or already facing them: leave the rotation alone on this every-frame path.
                                    if dx == 0.0 || lr == PostureModule::lr(module) {
                                        continue;
                                    }
                                    crate::slight::diag::note(format!(
                                        "face opponent {}: lr {lr}",
                                        actor.fighter
                                    ));
                                    lr
                                }
                            };
                            if !matches!(lr, -1.0 | 1.0) {
                                continue;
                            }
                            if check(boid, &actor.fighter, identity).is_err() {
                                return;
                            }
                            PostureModule::set_lr(module, lr);
                            if check(boid, &actor.fighter, identity).is_err() {
                                return;
                            }
                            PostureModule::update_rot_y_lr(module);
                        }
                    }
                }
                Command::AddDamage(amount) => {
                    let current_damage = DamageModule::damage(module, 0);
                    if check(boid, &actor.fighter, identity).is_err() {
                        return;
                    }
                    let Ok(delta) = super::damage_commands::damage_delta(current_damage, amount)
                    else {
                        continue;
                    };
                    DamageModule::add_damage(module, delta, 0);
                }
                Command::Combat(_, CombatOp::Clear(channel)) => {
                    if restore(
                        boid,
                        &actor.fighter,
                        identity,
                        super::damage_commands::Touched::channel(channel),
                    )
                    .is_err()
                    {
                        return;
                    }
                }
                Command::Combat(scope, op) => {
                    let armor = |mode: i32, threshold: f32| {
                        DamageModule::set_no_reaction_mode_status(
                            module,
                            DamageNoReactionMode {
                                _address: mode as u8,
                            },
                            -1.0,
                            threshold,
                            -1,
                        )
                    };
                    match op {
                        CombatOp::DamageTaken(value) => DamageModule::set_damage_mul(module, value),
                        CombatOp::KnockbackTaken(value) => {
                            DamageModule::set_reaction_mul(module, value)
                        }
                        CombatOp::DamageDealt(value) => AttackModule::set_power_mul(module, value),
                        CombatOp::KnockbackDealt(value) => {
                            AttackModule::set_reaction_mul(module, value)
                        }
                        CombatOp::Stat(stat, value) => super::live_parameters::set_stat(
                            boid,
                            identity.stock_generation,
                            stat,
                            value,
                        ),
                        // HDR plizardon/status/special_s.rs: DAMAGE_POWER's threshold
                        // is the SECOND float. The first float is reaction threshold.
                        CombatOp::Armor(threshold) => {
                            armor(*DAMAGE_NO_REACTION_MODE_DAMAGE_POWER, threshold)
                        }
                        CombatOp::SuperArmor => armor(*DAMAGE_NO_REACTION_MODE_ALWAYS, -1.0),
                        // The lua_bind behind vanilla WHOLE_HIT(HIT_STATUS_XLU / _INVINCIBLE).
                        CombatOp::Intangible => {
                            HitModule::set_whole(module, HitStatus(*HIT_STATUS_XLU), 0)
                        }
                        CombatOp::Invincible => {
                            HitModule::set_whole(module, HitStatus(*HIT_STATUS_INVINCIBLE), 0)
                        }
                        CombatOp::Clear(_) => unreachable!("handled above"),
                    }
                    crate::slight::diag::note(format!(
                        "combat policy object={boid} {} {:?} for {}",
                        op.channel().label(),
                        op,
                        scope.label()
                    ));
                }
                // HDR murabito/reflet: have_item(kind, 0, 0, false, false);
                // donkey throws: throw_item(angle, power, 1.0, 0, true, 0.0).
                Command::HoldItem(kind) => {
                    if !ItemModule::is_have_item(module, 0) {
                        ItemModule::have_item(module, ItemKind(kind), 0, 0, false, false);
                    }
                }
                Command::ThrowItem { angle, power } => {
                    if ItemModule::is_have_item(module, 0) {
                        ItemModule::throw_item(module, angle, power, 1.0, 0, true, 0.0);
                    }
                }
                Command::DropItem => {
                    if ItemModule::is_have_item(module, 0) {
                        ItemModule::drop_item(module, 90.0, 0.0, 0);
                    }
                }
                Command::RemoveItem => {
                    if ItemModule::is_have_item(module, 0) {
                        ItemModule::remove_item(module, 0);
                    }
                }
                // HDR daisy/status/attack_s4.rs. Often sent every frame, so only a change is logged, a bounded number of times.
                Command::MeshVisibility { mesh, visible } => {
                    ModelModule::set_mesh_visibility(
                        module,
                        smash::phx::Hash40::new_raw(mesh),
                        visible,
                    );
                    if LAST_MESH.swap(mesh << 1 | visible as u64, Ordering::Relaxed)
                        != (mesh << 1 | visible as u64)
                        && MESH_NOTES.fetch_add(1, Ordering::Relaxed) < 64
                    {
                        crate::slight::diag::note(format!(
                            "mesh object={boid} {mesh:#x} visible={visible}"
                        ));
                    }
                }
                _ => continue,
            }
        }
        actor.touched.record(command);
        if check(boid, &actor.fighter, identity).is_err() {
            return;
        }
    }
    let touched = actor.touched;
    let fighter = actor.fighter.clone();
    if QUEUE.lock().retain(actor).is_err() {
        let _ = restore(boid, &fighter, identity, touched);
    }
}
