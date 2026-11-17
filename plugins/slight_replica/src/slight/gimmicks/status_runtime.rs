//! Installed native phase adapters for controller-owned existing statuses.
use super::{
    actor_identity::Identity,
    live_model::{Action, Expr, Value},
    native_inputs::{PhaseInput, SituationKind},
    status_commands::{self as commands, Command, MotionFrame},
    status_model::Phase,
};
use smash::{
    app::lua_bind::*,
    lib::{lua_const::*, L2CValue},
    lua2cpp::L2CFighterCommon,
    phx::Hash40,
};

#[path = "../../../../../src/gimmicks/attack_damage.rs"]
mod attack_damage;
static ATTACK_DAMAGE: std::sync::LazyLock<parking_lot::Mutex<attack_damage::Pending<Identity>>> =
    std::sync::LazyLock::new(Default::default);

pub unsafe fn apply_attack_damage(module: *mut smash::app::BattleObjectModuleAccessor, id: u8) {
    if module.is_null() {
        return;
    }
    let boid = (*module).battle_object_id;
    let Some(record) = crate::slight::agents::lookup(boid) else {
        return;
    };
    let Some(fighter) = super::registry::fighter_name(record.kind) else {
        return;
    };
    let Some(identity) = Identity::from_live_fighter(boid, fighter) else {
        return;
    };
    let damage = ATTACK_DAMAGE
        .try_lock()
        .and_then(|pending| pending.get(boid, &identity, StatusModule::status_kind(module), id));
    if let Some(damage) = damage {
        if AttackModule::is_attack(module, id as i32, false) {
            AttackModule::set_power(module, id as i32, damage, false);
            crate::slight::diag::note(format!(
                "status hitbox damage object={boid} status={} id={id} damage={damage}",
                StatusModule::status_kind(module)
            ));
        }
    }
}

unsafe fn original(f: &mut L2CFighterCommon, bound: i32, phase: Phase) -> L2CValue {
    use std::{
        ptr::NonNull,
        sync::atomic::{AtomicUsize, Ordering},
    };
    type Lookup = extern "C" fn(
        &mut smash::lua2cpp::L2CFighterBase,
        smashline::StatusLine,
        i32,
    ) -> Option<NonNull<()>>;
    static ADDRESS: AtomicUsize = AtomicUsize::new(0);
    let mut address = ADDRESS.load(Ordering::Acquire);
    if address == 0 {
        if skyline::nn::ro::LookupSymbol(&mut address, b"smashline_get_original_status\0".as_ptr())
            != 0
            || address == 0
        {
            return 0.into();
        }
        ADDRESS.store(address, Ordering::Release);
    }
    let lookup: Lookup = std::mem::transmute(address);
    let line = match phase {
        Phase::Pre => smashline::StatusLine::Pre,
        Phase::Init => smashline::StatusLine::Init,
        Phase::MainEnter => smashline::StatusLine::Main,
        Phase::Exec => smashline::StatusLine::Exec,
        Phase::End => smashline::StatusLine::End,
        Phase::Exit => smashline::StatusLine::Exit,
        Phase::MainLoop => return 0.into(),
    };
    let Some(callback) = lookup(&mut *(f as *mut L2CFighterCommon).cast(), line, bound) else {
        return 0.into();
    };
    let callback: unsafe extern "C" fn(&mut L2CFighterCommon) -> L2CValue =
        std::mem::transmute(callback.as_ptr());
    callback(f)
}

pub unsafe fn dispatch(f: &mut L2CFighterCommon, bound: i32, phase: Phase) -> L2CValue {
    if phase == Phase::Pre && !f.module_accessor.is_null() {
        ATTACK_DAMAGE
            .lock()
            .clear((*f.module_accessor).battle_object_id);
    }
    // Read before the lease ends; STATUS_KIND (0xB) already names the next status at Exit.
    let leaving = (phase == Phase::Exit
        && !f.module_accessor.is_null()
        && super::live_behavior::owns_status((*f.module_accessor).battle_object_id, bound))
    .then(|| {
        let next = f.global_table[0xB].get_i32();
        (
            next,
            super::live_behavior::owns_status_of_chart((*f.module_accessor).battle_object_id, next),
        )
    });
    let outcome = run(f, bound, phase);
    if let Some((next, next_owned)) = leaving {
        super::grab_control::owned_status_exit(
            (*f.module_accessor).battle_object_id,
            next,
            next_owned,
        );
    }
    if phase == Phase::Exit && !f.module_accessor.is_null() {
        ATTACK_DAMAGE
            .lock()
            .clear((*f.module_accessor).battle_object_id);
        super::deflect_volume::set((*f.module_accessor).battle_object_id, None);
    }
    match outcome {
        Ok(None) => {
            let custom = crate::slight::agents::lookup((*f.module_accessor).battle_object_id)
                .and_then(|r| super::registry::fighter_name(r.kind))
                .and_then(|name| super::status_model::native_count(name).ok())
                .is_some_and(|num| bound as u32 > num);
            if custom {
                match phase {
                    Phase::Pre => {
                        initialize(f.module_accessor, &Default::default());
                        0.into()
                    }
                    Phase::MainEnter | Phase::MainLoop => recover(f),
                    _ => 0.into(),
                }
            } else {
                original(f, bound, phase)
            }
        }
        Ok(Some(changed)) => {
            if phase == Phase::MainEnter {
                return f.sub_shift_status_main(L2CValue::Ptr(main_loop as *const () as _));
            }
            i32::from(changed).into()
        }
        Err(error) => {
            crate::slight::diag::note(format!("status {bound} {phase:?}: {error}"));
            if matches!(phase, Phase::MainEnter | Phase::MainLoop) {
                return recover(f);
            }
            if phase == Phase::Pre {
                super::live_behavior::abort_status_pre(
                    (*f.module_accessor).battle_object_id,
                    bound,
                );
            }
            // Before an ownership lease exists, the original phase remains safe.
            if !super::live_behavior::owns_status((*f.module_accessor).battle_object_id, bound) {
                original(f, bound, phase)
            } else {
                0.into()
            }
        }
    }
}

unsafe extern "C" fn main_loop(f: &mut L2CFighterCommon) -> L2CValue {
    let status = StatusModule::status_kind(f.module_accessor);
    match run(f, status, Phase::MainLoop) {
        Ok(Some(changed)) => i32::from(changed).into(),
        Ok(None) => recover(f),
        Err(error) => {
            crate::slight::diag::note(format!("status loop {status}: {error}"));
            recover(f)
        }
    }
}
unsafe fn recover(f: &mut L2CFighterCommon) -> L2CValue {
    let status = if StatusModule::situation_kind(f.module_accessor) == *SITUATION_KIND_AIR {
        *FIGHTER_STATUS_KIND_FALL
    } else {
        *FIGHTER_STATUS_KIND_WAIT
    };
    f.change_status(status.into(), false.into());
    1.into()
}

unsafe fn run(f: &mut L2CFighterCommon, bound: i32, phase: Phase) -> Result<Option<bool>, String> {
    let module = f.module_accessor;
    if module.is_null() {
        return Err("status fighter context is unavailable".into());
    }
    let boid = (*module).battle_object_id;
    let Some(record) = crate::slight::agents::lookup(boid) else {
        return Ok(None);
    };
    let fighter =
        super::registry::fighter_name(record.kind).ok_or("status fighter kind is unknown")?;
    let Some((identity, inputs, uses_controller)) =
        super::live_behavior::status_inputs(boid, fighter, bound, phase)?
    else {
        return Ok(None);
    };
    if super::live_behavior::owns_status(boid, bound)
        && matches!(phase, Phase::MainLoop | Phase::Exec)
        && StopModule::is_stop(module)
    {
        return Ok(Some(false));
    }
    let mut snapshot = super::native_values::sample_behavior_inputs_guarded(
        boid,
        fighter,
        identity,
        &inputs,
        || require(boid, fighter, identity),
    )?;
    snapshot.phase = Some(PhaseInput {
        fighter: fighter.into(),
        status: bound,
        status_frame: f.global_table[0xE].get_i32(),
        motion_frame: MotionModule::frame(module),
        motion: MotionModule::motion_kind(module),
        situation: match StatusModule::situation_kind(module) {
            x if x == *SITUATION_KIND_GROUND => Some(SituationKind::Ground),
            x if x == *SITUATION_KIND_AIR => Some(SituationKind::Air),
            _ => None,
        },
    });
    snapshot.stopped = StopModule::is_stop(module);
    snapshot.situation_changed =
        Some(StatusModule::situation_kind(module) != StatusModule::prev_situation_kind(module));
    snapshot.motion_ended = Some(MotionModule::is_end(module));
    snapshot.cancel_enabled = Some(CancelModule::is_enable_cancel(module));
    for entry in super::registry::ENTRIES.iter().filter(|e| {
        e.kind == super::registry::IdentifierKind::TransitionTerm
            && super::registry::owned_by(e, fighter)
    }) {
        snapshot.term_enabled.insert(
            entry.symbol.into(),
            WorkModule::is_enable_transition_term(module, entry.value as i32),
        );
    }
    if uses_controller && phase != Phase::Pre {
        snapshot.controller = Some(super::native_values::sample_controller_input(
            boid, fighter, identity,
        )?);
    }
    require(boid, fighter, identity)?;
    let result = super::live_behavior::status_phase(
        boid,
        fighter,
        bound,
        phase,
        &snapshot,
        |action| {
            require(boid, fighter, identity)?;
            preflight(module, fighter, action)?;
            require(boid, fighter, identity)
        },
        |action| {
            require(boid, fighter, identity)?;
            execute(f, fighter, action)
        },
    );
    if matches!(result, Ok(Some(false)))
        && (phase == Phase::MainEnter
            || (phase == Phase::MainLoop && snapshot.situation_changed == Some(true)))
    {
        require(boid, fighter, identity)?;
        crate::slight::diag::note(format!("status setup {fighter}/{bound} {phase:?} situation={} motion={:x} frame={}->{} speed=({}, {})",StatusModule::situation_kind(module),MotionModule::motion_kind(module),snapshot.phase.as_ref().map_or(0.0,|p|p.motion_frame),MotionModule::frame(module),KineticModule::get_sum_speed_x(module,*KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN),KineticModule::get_sum_speed_y(module,*KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN)));
    }
    result
}
fn require(boid: u32, fighter: &str, identity: Identity) -> Result<(), String> {
    if Identity::from_live_fighter(boid, fighter) != Some(identity) {
        Err("status actor lifetime changed".into())
    } else {
        Ok(())
    }
}
fn scalar(expr: &Expr) -> Result<f32, String> {
    match expr {
        Expr::Const(Value::Float(v)) if v.is_finite() => Ok(*v as f32),
        Expr::Const(Value::Int(v)) => Ok(*v as f32),
        _ => Err("status argument was not resolved".into()),
    }
}
unsafe fn preflight(
    module: *mut smash::app::BattleObjectModuleAccessor,
    fighter: &str,
    action: &Action,
) -> Result<(), String> {
    let Action::Status(command) = action else {
        return Ok(());
    };
    commands::validate(command, fighter)?;
    if let Command::ConfigureEnergy { energy, .. } = command {
        if KineticModule::get_energy(module, energy_id(*energy)) == 0 {
            return Err("kinetic energy is unavailable".into());
        }
    }
    if let Command::PlayMotion {
        motion,
        frame,
        rate,
    } = command
    {
        if !MotionModule::is_anim_resource(module, Hash40::new(motion)) {
            return Err(format!("motion {motion:?} is unavailable for {fighter}"));
        }
        if !(0.05..=20.0).contains(&scalar(rate)?) {
            return Err("motion rate outside bounds".into());
        }
        if let MotionFrame::Start(frame) = frame {
            if !(0.0..=100000.0).contains(&scalar(frame)?) {
                return Err("motion start frame outside bounds".into());
            }
        }
    }
    Ok(())
}
/// The Home-Run Bat reflector call (vanilla BatSwing4), the one reflector
/// that needs no fighter-specific shape data.
unsafe fn volume(
    f: &mut L2CFighterCommon,
    enabled: bool,
    absorb: bool,
    size: f32,
    damage: f32,
    speed: f32,
) {
    use smash_script::{lua_args, shield};
    let boid = (*f.module_accessor).battle_object_id;
    if enabled {
        super::deflect_volume::set(boid, Some(absorb));
        shield!(
            f,
            *MA_MSC_CMD_REFLECTOR,
            *COLLISION_KIND_REFLECTOR,
            0,
            Hash40::new("top"),
            size,
            0.0,
            9.0,
            0.0,
            0.0,
            9.0,
            0.0,
            damage,
            speed,
            50,
            false,
            1.0,
            *FIGHTER_REFLECTOR_GROUP_HOMERUNBAT
        );
        crate::slight::diag::note(format!(
            "deflect volume object={boid} team={} hit_team={} damage%={}",
            TeamModule::team_no(f.module_accessor),
            TeamModule::hit_team_no(f.module_accessor),
            DamageModule::damage(f.module_accessor, 0)
        ));
    } else {
        shield!(
            f,
            *MA_MSC_CMD_SHIELD_OFF,
            *COLLISION_KIND_REFLECTOR,
            0,
            *FIGHTER_REFLECTOR_GROUP_HOMERUNBAT
        );
        super::deflect_volume::set(boid, None);
    }
}
/// Same call as vanilla CATCH (smash_script macros.rs); HDR issues grab commands from status code.
unsafe fn grab_box(f: &mut L2CFighterCommon, enabled: bool, size: f32, height: f32, forward: f32) {
    use smash_script::lua_args;
    if enabled {
        GrabModule::set_rebound(f.module_accessor, true);
        f.clear_lua_stack();
        lua_args!(f, 0i32, Hash40::new("top"), size, 0.0f32, height, forward);
        for _ in 0..3 {
            f.push_lua_stack(&mut L2CValue::new());
        }
        lua_args!(
            f,
            *FIGHTER_STATUS_KIND_CAPTURE_PULLED,
            *COLLISION_SITUATION_MASK_GA
        );
        smash::app::sv_animcmd::CATCH(f.lua_state_agent);
        f.clear_lua_stack();
    } else {
        GrabModule::clear_all(f.module_accessor);
        GrabModule::set_rebound(f.module_accessor, false);
    }
    crate::slight::diag::note(format!(
        "grab box object={} enabled={enabled} status={}",
        (*f.module_accessor).battle_object_id,
        StatusModule::status_kind(f.module_accessor)
    ));
}
unsafe fn execute(
    f: &mut L2CFighterCommon,
    fighter: &str,
    action: &Action,
) -> Result<bool, String> {
    let Action::Status(command) = action else {
        return Ok(false);
    };
    let module = f.module_accessor;
    match command {
        Command::Reflector {
            enabled,
            size,
            damage,
            speed,
        } => volume(f, *enabled, false, *size, *damage, *speed),
        // Same volume; deflect_volume turns its contacts into absorbs.
        Command::Absorber { enabled, size } => volume(f, *enabled, true, *size, 1.0, 1.0),
        Command::GrabBox {
            enabled,
            size,
            height,
            forward,
        } => grab_box(f, *enabled, *size, *height, *forward),
        Command::SetAttackDamage { id, damage } => {
            let damage = scalar(damage)?;
            if *id > 7 || !damage.is_finite() || !(0.0..=999.0).contains(&damage) {
                return Err("invalid hitbox damage".into());
            }
            let boid = (*module).battle_object_id;
            let identity = Identity::from_live_fighter(boid, fighter)
                .ok_or("Hitbox damage actor unavailable")?;
            ATTACK_DAMAGE.lock().set(
                boid,
                identity,
                StatusModule::status_kind(module),
                *id,
                damage,
            )?;
            if AttackModule::is_attack(module, *id as i32, false) {
                AttackModule::set_power(module, *id as i32, damage, false);
            }
        }
        Command::TransitionTerm { symbol, enabled }
        | Command::TransitionGroup { symbol, enabled } => {
            let group = matches!(command, Command::TransitionGroup { .. });
            let kind = if group {
                super::registry::IdentifierKind::TransitionGroup
            } else {
                super::registry::IdentifierKind::TransitionTerm
            };
            let id = super::registry::resolve(fighter, symbol, kind)?.value as i32;
            match (group, enabled) {
                (false, true) => WorkModule::enable_transition_term(module, id),
                (false, false) => WorkModule::unable_transition_term(module, id),
                (true, true) => WorkModule::enable_transition_term_group(module, id),
                (true, false) => WorkModule::unable_transition_term_group(module, id),
            }
        }
        Command::EnableCancel => {
            CancelModule::enable_cancel(module);
        }
        Command::TryCommonTransition { kind } => {
            use commands::CommonTransition as C;
            let before = StatusModule::status_kind(module);
            let handled = match kind {
                C::AirCliff => f.sub_transition_group_check_air_cliff(),
                C::GroundWait => f.sub_wait_ground_check_common(false.into()),
                C::AirFall => f.sub_air_check_fall_common(),
                C::AirLanding => f.sub_transition_group_check_air_landing(),
                C::AirSpecial => f.sub_transition_group_check_air_special(),
            }
            .get_bool();
            let terminal = handled
                || StatusModule::status_kind(module) != before
                || StatusModule::is_changing(module);
            if terminal {
                crate::slight::diag::note(format!(
                    "status common {fighter}/{before} {kind:?} handled={handled} motion_frame={}",
                    MotionModule::frame(module)
                ));
            }
            return Ok(terminal);
        }
        Command::ChangeKinetic { kind } => {
            KineticModule::change_kinetic(module, kinetic_id(*kind));
        }
        Command::SetSituation { situation } => match situation {
            commands::Situation::Keep => {}
            commands::Situation::Ground => StatusModule::set_situation_kind(
                module,
                smash::app::SituationKind(*SITUATION_KIND_GROUND),
                false,
            ),
            commands::Situation::Air => StatusModule::set_situation_kind(
                module,
                smash::app::SituationKind(*SITUATION_KIND_AIR),
                false,
            ),
        },
        Command::SetGroundPolicy { correction, cliff } => {
            if *correction != commands::Correction::Keep {
                GroundModule::correct(
                    module,
                    smash::app::GroundCorrectKind(correction_id(*correction)),
                );
            }
            GroundModule::set_cliff_check(
                module,
                smash::app::GroundCliffCheckKind(cliff_id(*cliff)),
            );
        }
        Command::ConfigureEnergy { energy, op } => configure_energy(f, *energy, op)?,
        Command::InitializeStatus(settings) => initialize(module, settings),
        Command::PlayMotion {
            motion,
            frame,
            rate,
        } => match frame {
            MotionFrame::Start(frame) => {
                MotionModule::change_motion(
                    module,
                    Hash40::new(motion),
                    scalar(frame)?,
                    scalar(rate)?,
                    false,
                    0.0,
                    false,
                    false,
                );
            }
            MotionFrame::Inherit => {
                MotionModule::change_motion_inherit_frame(
                    module,
                    Hash40::new(motion),
                    -1.0,
                    scalar(rate)?,
                    0.0,
                    false,
                    false,
                );
            }
        },
        Command::RequestStatus { target } => {
            super::operations::request_resolved_status(
                fighter,
                (*module).battle_object_id,
                target.resolve(fighter)?,
            )?;
            return Ok(true);
        }
        Command::ChangeStatusNow { symbol } => {
            let id = symbol.resolve(fighter)?;
            f.change_status((id as i32).into(), false.into());
        }
    }
    Ok(false)
}

unsafe fn initialize(module: *mut smash::app::BattleObjectModuleAccessor, s: &commands::Settings) {
    use commands::*;
    let situation = match s.situation {
        Situation::Keep => *SITUATION_KIND_NONE,
        Situation::Ground => *SITUATION_KIND_GROUND,
        Situation::Air => *SITUATION_KIND_AIR,
    };
    let kinetic = kinetic_id(s.kinetic);
    let correction = correction_id(s.correction);
    let cliff = cliff_id(s.cliff);
    let flags = match s.keep_flags {
        WorkKeep::None => *FIGHTER_STATUS_WORK_KEEP_FLAG_NONE_FLAG,
        WorkKeep::All => *FIGHTER_STATUS_WORK_KEEP_FLAG_ALL_FLAG,
    };
    let ints = match s.keep_ints {
        WorkKeep::None => *FIGHTER_STATUS_WORK_KEEP_FLAG_NONE_INT,
        WorkKeep::All => *FIGHTER_STATUS_WORK_KEEP_FLAG_ALL_INT,
    };
    let floats = match s.keep_floats {
        WorkKeep::None => *FIGHTER_STATUS_WORK_KEEP_FLAG_NONE_FLOAT,
        WorkKeep::All => *FIGHTER_STATUS_WORK_KEEP_FLAG_ALL_FLOAT,
    };
    StatusModule::init_settings(
        module,
        smash::app::SituationKind(situation),
        kinetic,
        correction as u32,
        smash::app::GroundCliffCheckKind(cliff),
        s.jostle,
        flags,
        ints,
        floats,
        0,
    );
    let d = &s.status_data;
    let tread = match d.tread {
        Tread::NoReaction => *FIGHTER_TREADED_KIND_NO_REAC,
        Tread::Enable => *FIGHTER_TREADED_KIND_ENABLE,
        Tread::Disable => *FIGHTER_TREADED_KIND_DISABLE,
    };
    let (kind, power) = match d.special {
        SpecialKind::Neutral => (
            *FIGHTER_LOG_MASK_FLAG_ATTACK_KIND_SPECIAL_N,
            *FIGHTER_POWER_UP_ATTACK_BIT_SPECIAL_N,
        ),
        SpecialKind::Side => (
            *FIGHTER_LOG_MASK_FLAG_ATTACK_KIND_SPECIAL_S,
            *FIGHTER_POWER_UP_ATTACK_BIT_SPECIAL_S,
        ),
        SpecialKind::Up => (
            *FIGHTER_LOG_MASK_FLAG_ATTACK_KIND_SPECIAL_HI,
            *FIGHTER_POWER_UP_ATTACK_BIT_SPECIAL_HI,
        ),
        SpecialKind::Down => (
            *FIGHTER_LOG_MASK_FLAG_ATTACK_KIND_SPECIAL_LW,
            *FIGHTER_POWER_UP_ATTACK_BIT_SPECIAL_LW,
        ),
    };
    let mut log = kind as u64;
    if d.log_attack {
        log |= *FIGHTER_LOG_MASK_FLAG_ACTION_CATEGORY_ATTACK as u64;
    }
    if d.log_action_trigger {
        log |= *FIGHTER_LOG_MASK_FLAG_ACTION_TRIGGER_ON as u64;
    }
    FighterStatusModuleImpl::set_fighter_status_data(
        module,
        d.sdk_arg2,
        tread,
        d.sdk_arg4,
        d.grab_invulnerable,
        d.sdk_arg6,
        log,
        if d.start_turn {
            *FIGHTER_STATUS_ATTR_START_TURN as u32
        } else {
            0
        },
        if d.power_up_special { power as u32 } else { 0 },
        0,
    );
}

pub(super) unsafe fn energy_id(energy: commands::Energy) -> i32 {
    match energy {
        commands::Energy::Stop => *FIGHTER_KINETIC_ENERGY_ID_STOP,
        commands::Energy::Gravity => *FIGHTER_KINETIC_ENERGY_ID_GRAVITY,
        commands::Energy::Control => *FIGHTER_KINETIC_ENERGY_ID_CONTROL,
        commands::Energy::Motion => *FIGHTER_KINETIC_ENERGY_ID_MOTION,
    }
}
unsafe fn kinetic_id(kind: commands::Kinetic) -> i32 {
    match kind {
        commands::Kinetic::Unique => *FIGHTER_KINETIC_TYPE_UNIQ,
        commands::Kinetic::GroundStop => *FIGHTER_KINETIC_TYPE_GROUND_STOP,
        commands::Kinetic::Fall => *FIGHTER_KINETIC_TYPE_FALL,
    }
}
unsafe fn correction_id(kind: commands::Correction) -> i32 {
    match kind {
        commands::Correction::Keep => *GROUND_CORRECT_KIND_KEEP,
        commands::Correction::Ground => *GROUND_CORRECT_KIND_GROUND,
        commands::Correction::Air => *GROUND_CORRECT_KIND_AIR,
    }
}
unsafe fn cliff_id(kind: commands::Cliff) -> i32 {
    match kind {
        commands::Cliff::None => *GROUND_CLIFF_CHECK_KIND_NONE,
        commands::Cliff::Always => *GROUND_CLIFF_CHECK_KIND_ALWAYS,
        commands::Cliff::AlwaysBothSides => *GROUND_CLIFF_CHECK_KIND_ALWAYS_BOTH_SIDES,
    }
}
unsafe fn configure_energy(
    f: &mut L2CFighterCommon,
    energy: commands::Energy,
    op: &commands::EnergyOp<Expr>,
) -> Result<(), String> {
    use commands::{EnergyOp as O, Reset as R};
    use smash_script::{lua_args, sv_kinetic_energy};
    let id = energy_id(energy);
    match op {
        O::Enable => {
            KineticModule::enable_energy(f.module_accessor, id);
        }
        O::Disable => {
            KineticModule::unable_energy(f.module_accessor, id);
        }
        O::Reset { kind, x, y } => {
            let reset = match kind {
                R::StopGround => *ENERGY_STOP_RESET_TYPE_GROUND,
                R::StopAir => *ENERGY_STOP_RESET_TYPE_AIR,
                R::Gravity => *ENERGY_GRAVITY_RESET_TYPE_GRAVITY,
                R::ControlFallAdjust => *ENERGY_CONTROLLER_RESET_TYPE_FALL_ADJUST,
                R::MotionGround => *ENERGY_MOTION_RESET_TYPE_GROUND_TRANS_IGNORE_NORMAL,
                R::MotionAir => *ENERGY_MOTION_RESET_TYPE_AIR_TRANS,
            };
            sv_kinetic_energy!(
                reset_energy,
                f,
                id,
                reset,
                scalar(x)?,
                scalar(y)?,
                0.0,
                0.0,
                0.0
            );
        }
        O::Speed { x, y } => {
            if energy == commands::Energy::Gravity {
                sv_kinetic_energy!(set_speed, f, id, scalar(y)?);
            } else {
                sv_kinetic_energy!(set_speed, f, id, scalar(x)?, scalar(y)?);
            }
        }
        O::Accel { x, y } => {
            if energy == commands::Energy::Gravity {
                sv_kinetic_energy!(set_accel, f, id, scalar(y)?);
            } else {
                sv_kinetic_energy!(set_accel, f, id, scalar(x)?, scalar(y)?);
            }
        }
        O::Brake { x, y } => {
            if energy == commands::Energy::Gravity {
                sv_kinetic_energy!(set_brake, f, id, scalar(y)?);
            } else {
                sv_kinetic_energy!(set_brake, f, id, scalar(x)?, scalar(y)?);
            }
        }
        O::Limit { x, y } => {
            if energy == commands::Energy::Gravity {
                sv_kinetic_energy!(set_limit_speed, f, id, scalar(y)?);
            } else {
                sv_kinetic_energy!(set_limit_speed, f, id, scalar(x)?, scalar(y)?);
            }
        }
        O::Stable { x, y } => {
            if energy == commands::Energy::Gravity {
                sv_kinetic_energy!(set_stable_speed, f, id, scalar(y)?);
            } else {
                sv_kinetic_energy!(set_stable_speed, f, id, scalar(x)?, scalar(y)?);
            }
        }
    }
    Ok(())
}
