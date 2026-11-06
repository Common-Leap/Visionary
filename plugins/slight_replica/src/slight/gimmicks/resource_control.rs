//! Native resource rows (`resource_commands::RESOURCES`) through one prepared-action boundary.
//! The row's writer picks the fighter's own helper, or a plain Work write.
use super::{
    actor_identity::Identity,
    resource_commands::{Command, Writer},
};
use smash::app::{self, lua_bind::WorkModule};

/// The evaluator resolves every amount to a Float constant before delivery.
pub(crate) fn resolved(command: &Command<super::live_model::Expr>) -> Result<Command, String> {
    command.try_map(|amount| match amount {
        super::live_model::Expr::Const(super::live_model::Value::Float(v)) => Ok(*v),
        _ => Err("native resource amount was not resolved at its action".to_string()),
    })
}

fn current(boid: u32, expected: Identity, command: Command) -> Result<(), String> {
    if Identity::from_live_fighter(boid, command.resource.fighter()) != Some(expected) {
        return Err("resource actor identity changed".into());
    }
    Ok(())
}

pub(crate) fn preflight(boid: u32, expected: Identity, command: Command) -> Result<(), String> {
    checked(boid, expected, command).map(|_| ())
}

fn cloud(command: Command, value: f32, target: f32) -> super::cloud_resource::Command {
    if target == 0.0 && command.op == super::resource_commands::Op::Set {
        super::cloud_resource::Command::Clear
    } else {
        super::cloud_resource::Command::Adjust(target - value)
    }
}

/// `(current, target, work id)` after identity, status and bound checks.
fn checked(boid: u32, expected: Identity, command: Command) -> Result<(f32, f32, i32), String> {
    let entry = command.resource.entry();
    command.validate(entry.fighter)?;
    current(boid, expected, command)?;
    // A partner's damage lives on the partner, not in a Work field of this fighter.
    if matches!(entry.writer, Writer::PartnerAction(_)) {
        return Ok((0.0, 0.0, 0));
    }
    if entry.writer == Writer::PartnerDamage {
        let value = super::partner_control::damage(boid, entry.fighter, expected);
        return Ok((
            value.unwrap_or(0.0),
            command.target(value.unwrap_or(0.0))?,
            0,
        ));
    }
    if matches!(entry.writer, Writer::PickelWear(_)) {
        return Ok((0.0, 0.0, 0));
    }
    if let Writer::PickelTier(weapon) = entry.writer {
        let weapon = craft_weapon(weapon)?;
        let value = unsafe {
            let material = app::FighterSpecializer_Pickel::get_craft_weapon_material_kind(
                fighter(boid, expected, command)?,
                weapon,
            );
            TIERS
                .iter()
                .position(|tier| **tier == material)
                .unwrap_or(0) as f32
        };
        return Ok((value, command.target(value)?, 0));
    }
    let module = expected.module_accessor_addr as *mut app::BattleObjectModuleAccessor;
    let integer = entry.kind == super::native_inputs::NativeKind::Int;
    let kind = if integer {
        super::registry::IdentifierKind::WorkInt
    } else {
        super::registry::IdentifierKind::WorkFloat
    };
    let id = super::registry::work_value_id(entry.fighter, entry.symbol, kind)? as i32;
    let value = unsafe {
        if integer {
            WorkModule::get_int(module, id) as f32
        } else {
            WorkModule::get_float(module, id)
        }
    };
    let target = command.target(value)?;
    match entry.writer {
        // Plain Work rows are fields the game reads every frame; any status may write them.
        Writer::Work => {}
        // Cloud's helper carries its own reviewed phase gate.
        Writer::CloudLimit => {
            super::cloud_resource::preflight(boid, expected, cloud(command, value, target))?
        }
        // The game runs Steve's, Joker's, Hero's and Inkling's helpers during their own attacks,
        // so a rule may change these on a hit. Hero's and Inkling's need the real fighter object.
        Writer::HeroMp | Writer::InklingInk => unsafe {
            fighter(boid, expected, command)?;
        },
        _ => {}
    }
    current(boid, expected, command)?;
    Ok((value, target, id))
}

unsafe fn fighter(
    boid: u32,
    expected: Identity,
    command: Command,
) -> Result<*mut app::Fighter, String> {
    // Reuse phase_control's existing reviewed Lua context path, then the SDK's
    // typed BattleObject header. Never cast a module accessor to Fighter.
    let common = super::phase_control::fighter_common(boid, command.resource.fighter(), expected)?;
    let object = (&*common).battle_object;
    if object.is_null()
        || (*object).battle_object_id != boid
        || (*object).kind != expected.fighter_kind as u32
        || (*object).entry_id != expected.entry_id as u64
        || (*object).module_accessor as u64 != expected.module_accessor_addr
    {
        return Err("resource fighter context does not match actor".into());
    }
    current(boid, expected, command)?;
    Ok(object.cast())
}

/// Steve's tool materials, weakest first; a tier row's amount indexes this.
static TIERS: [&smash::lib::LuaConst; 5] = [
    &smash::lib::lua_const::FIGHTER_PICKEL_MATERIAL_KIND_WOOD,
    &smash::lib::lua_const::FIGHTER_PICKEL_MATERIAL_KIND_STONE,
    &smash::lib::lua_const::FIGHTER_PICKEL_MATERIAL_KIND_IRON,
    &smash::lib::lua_const::FIGHTER_PICKEL_MATERIAL_KIND_GOLD,
    &smash::lib::lua_const::FIGHTER_PICKEL_MATERIAL_KIND_DIAMOND,
];

fn craft_weapon(name: &str) -> Result<app::FighterPickelCraftWeaponKind, String> {
    use smash::lib::lua_const::*;
    Ok(app::FighterPickelCraftWeaponKind(match name {
        "FIGHTER_PICKEL_CRAFT_WEAPON_KIND_SWORD" => *FIGHTER_PICKEL_CRAFT_WEAPON_KIND_SWORD,
        "FIGHTER_PICKEL_CRAFT_WEAPON_KIND_PICK" => *FIGHTER_PICKEL_CRAFT_WEAPON_KIND_PICK,
        "FIGHTER_PICKEL_CRAFT_WEAPON_KIND_AXE" => *FIGHTER_PICKEL_CRAFT_WEAPON_KIND_AXE,
        "FIGHTER_PICKEL_CRAFT_WEAPON_KIND_SHOVEL" => *FIGHTER_PICKEL_CRAFT_WEAPON_KIND_SHOVEL,
        _ => return Err(format!("unknown Steve tool {name}")),
    }))
}

fn material(name: &str) -> Result<i32, String> {
    use smash::lib::lua_const::*;
    Ok(match name {
        "FIGHTER_PICKEL_MATERIAL_KIND_GRADE_1" => *FIGHTER_PICKEL_MATERIAL_KIND_GRADE_1,
        "FIGHTER_PICKEL_MATERIAL_KIND_WOOD" => *FIGHTER_PICKEL_MATERIAL_KIND_WOOD,
        "FIGHTER_PICKEL_MATERIAL_KIND_STONE" => *FIGHTER_PICKEL_MATERIAL_KIND_STONE,
        "FIGHTER_PICKEL_MATERIAL_KIND_IRON" => *FIGHTER_PICKEL_MATERIAL_KIND_IRON,
        "FIGHTER_PICKEL_MATERIAL_KIND_GOLD" => *FIGHTER_PICKEL_MATERIAL_KIND_GOLD,
        "FIGHTER_PICKEL_MATERIAL_KIND_DIAMOND" => *FIGHTER_PICKEL_MATERIAL_KIND_DIAMOND,
        "FIGHTER_PICKEL_MATERIAL_KIND_RED_STONE" => *FIGHTER_PICKEL_MATERIAL_KIND_RED_STONE,
        _ => return Err(format!("unknown Steve material {name}")),
    })
}

pub(crate) fn execute(boid: u32, expected: Identity, command: Command) -> Result<(), String> {
    let (value, target, id) = checked(boid, expected, command)?;
    let delta = target - value;
    let module = expected.module_accessor_addr as *mut app::BattleObjectModuleAccessor;
    unsafe {
        match command.resource.entry().writer {
            Writer::Work
                if command.resource.entry().kind == super::native_inputs::NativeKind::Int =>
            {
                WorkModule::set_int(module, target as i32, id)
            }
            Writer::Work => WorkModule::set_float(module, target, id),
            Writer::CloudLimit => {
                super::cloud_resource::execute(boid, expected, cloud(command, value, target))?;
            }
            Writer::RebelGauge => {
                app::FighterSpecializer_Jack::add_rebel_gauge(
                    module,
                    app::FighterEntryID(expected.entry_id),
                    delta,
                );
            }
            Writer::HeroMp => {
                app::FighterSpecializer_Brave::add_sp(fighter(boid, expected, command)?, delta);
            }
            Writer::InklingInk => {
                let fighter = fighter(boid, expected, command)?;
                let max = app::FighterSpecializer_Inkling::get_ink_max(fighter);
                if !max.is_finite() || max <= 0.0 {
                    return Err("invalid native ink capacity".into());
                }
                app::FighterSpecializer_Inkling::change_ink(fighter, target.min(max));
            }
            Writer::PartnerDamage => {
                super::partner_control::add_damage(
                    boid,
                    command.resource.fighter(),
                    expected,
                    delta,
                );
            }
            Writer::PartnerAction(status) => {
                super::partner_control::request_status(
                    boid,
                    command.resource.fighter(),
                    expected,
                    status,
                )?;
            }
            Writer::PickelWear(weapon) => {
                app::FighterSpecializer_Pickel::sub_craft_weapon_durability(
                    fighter(boid, expected, command)?,
                    craft_weapon(weapon)?,
                    command.amount.max(0.0),
                );
            }
            Writer::PickelTier(weapon) => {
                let steve = fighter(boid, expected, command)?;
                let weapon = craft_weapon(weapon)?;
                let tier = TIERS
                    .get(target.clamp(0.0, 4.0) as usize)
                    .ok_or("unknown Steve tool tier")?;
                app::FighterSpecializer_Pickel::set_craft_weapon_param(
                    steve,
                    weapon,
                    app::FighterPickelMaterialKind(***tier),
                    1.0,
                );
            }
            Writer::PickelMaterial(kind) => {
                let kind = material(kind)?;
                if delta >= 0.0 {
                    app::FighterSpecializer_Pickel::add_material_num(module, kind, delta as i32);
                } else {
                    app::FighterSpecializer_Pickel::sub_material_num(module, kind, -delta as i32);
                }
            }
        }
    }
    crate::slight::diag::note(format!(
        "resource {} object={boid} {value} -> {target}",
        command.resource.entry().key
    ));
    current(boid, expected, command)
}
