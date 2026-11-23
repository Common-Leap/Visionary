//! Immediate fighter phase reads and bounded motion/kinetic commands.
use super::{
    live_behavior::Identity,
    live_model as model,
    native_inputs::{PhaseInput, SituationKind},
};
use smash::app::lua_bind::{KineticModule, MotionModule, PostureModule, StatusModule};

fn require_identity(boid: u32, fighter: &str, expected: Identity) -> Result<(), String> {
    if Identity::from_live_fighter(boid, fighter) != Some(expected) {
        return Err("fighter identity changed during phase control".into());
    }
    Ok(())
}

pub(crate) fn sample(boid: u32, fighter: &str, expected: Identity) -> Result<PhaseInput, String> {
    require_identity(boid, fighter, expected)?;
    let module = expected.module_accessor_addr as *mut smash::app::BattleObjectModuleAccessor;
    let status_frame =
        unsafe { (&*fighter_common(boid, fighter, expected)?).global_table[0xE].get_i32() };
    require_identity(boid, fighter, expected)?;
    let status = unsafe { StatusModule::status_kind(module) };
    require_identity(boid, fighter, expected)?;
    let motion_frame = unsafe { MotionModule::frame(module) };
    require_identity(boid, fighter, expected)?;
    let motion = unsafe { MotionModule::motion_kind(module) };
    require_identity(boid, fighter, expected)?;
    let situation = unsafe { StatusModule::situation_kind(module) };
    require_identity(boid, fighter, expected)?;
    let situation = if situation == *smash::lib::lua_const::SITUATION_KIND_GROUND {
        Some(SituationKind::Ground)
    } else if situation == *smash::lib::lua_const::SITUATION_KIND_AIR {
        Some(SituationKind::Air)
    } else {
        None
    };
    let input = PhaseInput {
        fighter: fighter.into(),
        status,
        status_frame,
        motion_frame,
        motion,
        situation,
    };
    input.validate()?;
    Ok(input)
}

pub(crate) fn preflight(
    boid: u32,
    fighter: &str,
    expected: Identity,
    action: &model::Action,
) -> Result<(), String> {
    super::phase_commands::resolve(action)?;
    require_identity(boid, fighter, expected)
}
pub(crate) fn execute(
    boid: u32,
    fighter: &str,
    expected: Identity,
    action: &model::Action,
) -> Result<(), String> {
    preflight(boid, fighter, expected, action)?;
    let command = super::phase_commands::resolve(action)?;
    let module = expected.module_accessor_addr as *mut smash::app::BattleObjectModuleAccessor;
    if matches!(
        command,
        super::phase_commands::Command::HeldDamage(_)
            | super::phase_commands::Command::ThrowHeldOpponent(_)
            | super::phase_commands::Command::ReleaseHeldOpponent
    ) {
        return super::grab_control::enqueue(boid, fighter, expected, command);
    }
    if super::damage_commands::is_damage(command) {
        return super::damage_control::enqueue(boid, fighter, expected, command);
    }
    super::phase_commands::execute_guarded(
        command,
        || require_identity(boid, fighter, expected),
        || unsafe { PostureModule::lr(module) },
        |command| unsafe {
            match command {
                super::phase_commands::Command::Movement(_)
                | super::phase_commands::Command::HeldDamage(_)
                | super::phase_commands::Command::ThrowHeldOpponent(_)
                | super::phase_commands::Command::ReleaseHeldOpponent
                | super::phase_commands::Command::AddDamage(_)
                | super::phase_commands::Command::Combat(..)
                | super::phase_commands::Command::HoldItem(_)
                | super::phase_commands::Command::ThrowItem { .. }
                | super::phase_commands::Command::DropItem
                | super::phase_commands::Command::RemoveItem
                | super::phase_commands::Command::MeshVisibility { .. } => {
                    unreachable!("queued above")
                }
                super::phase_commands::Command::Rate(rate) => {
                    MotionModule::set_rate(module, rate);
                }
                super::phase_commands::Command::Speed { x, y } => {
                    KineticModule::add_speed(module, &smash::phx::Vector3f { x, y, z: 0.0 });
                }
            }
        },
    )
}

/// Existing reviewed Lua-module path, shared by phase reads and SDK helpers.
pub(crate) fn fighter_common(
    boid: u32,
    fighter: &str,
    expected: Identity,
) -> Result<*mut smash::lua2cpp::L2CFighterCommon, String> {
    require_identity(boid, fighter, expected)?;
    unsafe {
        let module = expected.module_accessor_addr as *const u64;
        let lua_module = *module.add(0x190 / 8);
        if lua_module == 0 {
            return Err("fighter Lua module is unavailable".into());
        }
        let common = *((lua_module + 0x1D8) as *const *mut smash::lua2cpp::L2CFighterCommon);
        if common.is_null() || (&*common).module_accessor as u64 != expected.module_accessor_addr {
            return Err("fighter status context is unavailable".into());
        }
        require_identity(boid, fighter, expected)?;
        Ok(common)
    }
}
