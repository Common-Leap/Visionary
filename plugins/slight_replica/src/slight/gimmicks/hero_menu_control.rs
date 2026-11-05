//! Hero Command Selection reads and bounded cursor selection.
//!
//! The menu owns its command list; this module only observes the highlighted
//! slot and moves the native cursor to a slot the UI already offers.
use super::{hero_menu_commands::Command, live_behavior::Identity};
use smash::app::{
    self,
    lua_bind::{StatusModule, WorkModule},
};

fn require_identity(boid: u32, fighter: &str, expected: Identity) -> Result<(), String> {
    if Identity::from_live_fighter(boid, fighter) != Some(expected) {
        return Err("hero menu identity changed".into());
    }
    Ok(())
}

fn menu_status() -> i32 {
    *smash::lib::lua_const::FIGHTER_BRAVE_STATUS_KIND_SPECIAL_LW_SELECT
}

fn select_index_id() -> i32 {
    *smash::lib::lua_const::FIGHTER_BRAVE_INSTANCE_WORK_ID_INT_SPECIAL_LW_SELECT_INDEX
}

fn list_max() -> i32 {
    *smash::lib::lua_const::FIGHTER_BRAVE_SPECIAL_LW_COMMAND_LIST_MAX
}

unsafe fn fighter_object(
    boid: u32,
    fighter: &str,
    expected: Identity,
) -> Result<*mut app::Fighter, String> {
    let common = super::phase_control::fighter_common(boid, fighter, expected)?;
    let object = (&*common).battle_object;
    if object.is_null()
        || (*object).battle_object_id != boid
        || (*object).kind != expected.fighter_kind as u32
        || (*object).entry_id != expected.entry_id as u64
        || (*object).module_accessor as u64 != expected.module_accessor_addr
    {
        return Err("hero menu fighter context does not match actor".into());
    }
    require_identity(boid, fighter, expected)?;
    Ok(object.cast())
}

fn menu_is_open(module: *mut app::BattleObjectModuleAccessor) -> bool {
    unsafe {
        StatusModule::status_kind(module) == menu_status() && !StatusModule::is_changing(module)
    }
}

/// Highlighted command kind, or `-1` when the menu is closed or its index is
/// outside the four native slots.
pub(crate) fn sample(boid: u32, fighter: &str, expected: Identity) -> Result<i32, String> {
    require_identity(boid, fighter, expected)?;
    let module = expected.module_accessor_addr as *mut app::BattleObjectModuleAccessor;
    if !menu_is_open(module) {
        return Ok(-1);
    }
    let index = unsafe { WorkModule::get_int(module, select_index_id()) };
    require_identity(boid, fighter, expected)?;
    if !(0..list_max()).contains(&index) {
        return Ok(-1);
    }
    let brave = unsafe { fighter_object(boid, fighter, expected)? };
    let command =
        unsafe { app::FighterSpecializer_Brave::get_special_lw_command_from_index(brave, index) };
    require_identity(boid, fighter, expected)?;
    Ok(command)
}

pub(crate) fn preflight(
    boid: u32,
    fighter: &str,
    expected: Identity,
    slot: u8,
) -> Result<(), String> {
    (Command::Choose { slot }).validate(fighter)?;
    require_identity(boid, fighter, expected)?;
    let module = expected.module_accessor_addr as *mut app::BattleObjectModuleAccessor;
    if !menu_is_open(module) {
        return Err("Hero menu is not open".into());
    }
    require_identity(boid, fighter, expected)
}

pub(crate) fn execute(
    boid: u32,
    fighter: &str,
    expected: Identity,
    slot: u8,
) -> Result<(), String> {
    preflight(boid, fighter, expected, slot)?;
    let module = expected.module_accessor_addr as *mut app::BattleObjectModuleAccessor;
    let index = i32::from(slot);
    unsafe {
        WorkModule::set_int(module, index, select_index_id());
    }
    let brave = unsafe { fighter_object(boid, fighter, expected)? };
    unsafe {
        app::FighterSpecializer_Brave::special_lw_select_index(brave, index);
    }
    require_identity(boid, fighter, expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_validation_is_closed() {
        assert_eq!((Command::Choose { slot: 0 }).validate("brave"), Ok(()));
        assert_eq!((Command::Choose { slot: 3 }).validate("brave"), Ok(()));
        assert!((Command::Choose { slot: 4 }).validate("brave").is_err());
        assert!((Command::Choose { slot: 0 }).validate("mario").is_err());
    }

    #[test]
    fn slot_range_matches_native_list() {
        assert_eq!(i32::from(Command::SLOTS), list_max());
    }
}
