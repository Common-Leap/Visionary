//! Read-only partner observations for Ice Climbers and Rosalina.
//!
//! Nana is a separate fighter object driven by Popo's player entry; Luma is
//! the rosetta_tico weapon this Rosalina owns. An absent or KO'd partner is a
//! defined observation (not present, zero damage), never an error.
use super::{
    actor_identity::Identity,
    native_inputs::{FighterValue, NativeInput, NativeObservation},
};
use smash::app::{
    lua_bind::{DamageModule, StatusModule},
    sv_battle_object, BattleObjectModuleAccessor,
};
use smash::lib::lua_const::*;

fn check_owner(boid: u32, fighter: &str, identity: Identity) -> Result<(), String> {
    if Identity::from_live_fighter(boid, fighter) != Some(identity) {
        return Err("partner owner identity changed".into());
    }
    Ok(())
}

/// The live, on-stage Nana sharing this player entry.
unsafe fn nana(entry_id: i32) -> Option<*mut BattleObjectModuleAccessor> {
    let kind = *FIGHTER_KIND_NANA;
    crate::slight::agents::all_records()
        .into_iter()
        .find_map(|rec| {
            if rec.category != *BATTLE_OBJECT_CATEGORY_FIGHTER
                || rec.kind != kind
                || rec.entry_id != entry_id
                || sv_battle_object::is_null(rec.boid)
                || !sv_battle_object::is_active(rec.boid)
                || sv_battle_object::category(rec.boid) != *BATTLE_OBJECT_CATEGORY_FIGHTER
                || sv_battle_object::kind(rec.boid) != kind
                || sv_battle_object::entry_id(rec.boid) != entry_id
            {
                return None;
            }
            let module = sv_battle_object::module_accessor(rec.boid);
            if module.is_null() || module as u64 != rec.module_accessor_addr {
                return None;
            }
            let status = StatusModule::status_kind(module);
            (status != *FIGHTER_STATUS_KIND_DEAD && status != *FIGHTER_STATUS_KIND_STANDBY)
                .then_some(module)
        })
}

/// The live, on-stage Luma owned by this Rosalina.
unsafe fn luma(boid: u32) -> Option<*mut BattleObjectModuleAccessor> {
    crate::slight::systems::article_notifier::owned_weapons(
        boid,
        "rosetta",
        "FIGHTER_ROSETTA_GENERATE_ARTICLE_TICO",
    )
    .into_iter()
    .find_map(|weapon| {
        let module = sv_battle_object::module_accessor(weapon);
        if module.is_null() {
            return None;
        }
        let status = StatusModule::status_kind(module);
        (status != *WEAPON_ROSETTA_TICO_STATUS_KIND_DEAD
            && status != *WEAPON_ROSETTA_TICO_STATUS_KIND_STANDBY
            && status != *WEAPON_ROSETTA_TICO_STATUS_KIND_NONE)
            .then_some(module)
    })
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
                value: FighterValue::PartnerPresent,
                ..
            } | NativeInput::Fighter {
                value: FighterValue::PartnerDamage,
                ..
            }
        )
    {
        return Err("unsupported or foreign partner source".into());
    }
    check_owner(boid, fighter, identity)?;
    let partner = unsafe {
        match fighter {
            "popo" => nana(identity.entry_id),
            "rosetta" => luma(boid),
            _ => return Err("fighter has no partner".into()),
        }
    };
    let observation = match (source, partner) {
        (
            NativeInput::Fighter {
                value: FighterValue::PartnerPresent,
                ..
            },
            partner,
        ) => NativeObservation::Flag(partner.is_some()),
        (_, None) => NativeObservation::Float(0.0),
        (_, Some(module)) => {
            let value = unsafe { DamageModule::damage(module, 0) };
            if !value.is_finite() || !(0.0..=1_000_000.0).contains(&value) {
                return Err("invalid partner damage value".into());
            }
            NativeObservation::Float(value)
        }
    };
    check_owner(boid, fighter, identity)?;
    Ok(observation)
}
