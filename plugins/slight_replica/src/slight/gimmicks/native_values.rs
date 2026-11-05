//! Identity-scoped, read-only native values.
//!
//! This is an immediate game-thread read, not telemetry, persistent behavior
//! state or an inferred resource event. Callers supply the dispatch identity;
//! the whole batch fails if ownership changes or any requested value is invalid.
//! Registry membership proves source spelling/type/ownership, not its meaning
//! or lifecycle. A graph input still needs a reviewed source and sampling policy.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicI32, Ordering};

use smash::app::lua_bind::{
    ArticleModule, AttackModule, DamageModule, FighterInformation, FighterManager, ItemModule,
    KineticModule, PostureModule, StatusModule, WorkModule,
};
use smash::lib::lua_const::{
    COLLISION_KIND_MASK_ABSORBER, COLLISION_KIND_MASK_HIT, COLLISION_KIND_MASK_REFLECTOR,
    COLLISION_KIND_MASK_SHIELD, FIGHTER_STATUS_KIND_DEAD, FIGHTER_STATUS_KIND_STANDBY,
};

use super::{
    behavior::NativeSource,
    live_behavior::Identity,
    native_inputs::{
        AttackResultMask, AttackResultQuery, FighterValue, NativeInput, NativeKind,
        NativeObservation, NativeSnapshot,
    },
    registry,
};

extern "C" {
    #[link_name = "_ZN3lib9SingletonIN3app14FighterManagerEE9instance_E"]
    static FIGHTER_MANAGER: *mut core::ffi::c_void;
}

unsafe fn sample_stocks(expected: Identity, opponent: bool) -> Result<i32, String> {
    let manager = FIGHTER_MANAGER.cast::<smash::app::FighterManager>();
    if manager.is_null() {
        return Err("fighter manager is unavailable".into());
    }
    let read = |entry_id| {
        let info =
            FighterManager::get_fighter_information(manager, smash::app::FighterEntryID(entry_id));
        if info.is_null() {
            return Err("fighter information is unavailable".to_owned());
        }
        i32::try_from(FighterInformation::stock_count(info))
            .map_err(|_| "stock count is outside the integer range".to_owned())
    };
    if !opponent {
        return read(expected.entry_id);
    }
    // Use the sole other active fighter in 1v1, otherwise the most stocks among other active fighters.
    let mut most = None;
    let mut entries = BTreeSet::new();
    for record in crate::slight::agents::all_records() {
        if record.entry_id == expected.entry_id || record.category != 0 {
            continue;
        }
        let Some(name) = registry::fighter_name(record.kind) else {
            continue;
        };
        let Some(identity) = Identity::from_live_fighter(record.boid, name) else {
            continue;
        };
        if !entries.insert(identity.entry_id) {
            continue;
        }
        let stocks = read(identity.entry_id)?;
        require_identity(record.boid, name, identity)?;
        most = Some(most.map_or(stocks, |value: i32| value.max(stocks)));
    }
    most.ok_or_else(|| "opponent stocks are unavailable without another active fighter".into())
}

/// The opponent `FighterValue::OpponentPresent` defines, resolved afresh for each
/// reading behind the same live-object fence the stock lookup uses. A shared
/// player entry (Popo and Nana) contributes whichever of its fighters is nearer.
pub(crate) unsafe fn nearest_opponent(
    expected: Identity,
) -> Option<*mut smash::app::BattleObjectModuleAccessor> {
    let me = expected.module_accessor_addr as *mut smash::app::BattleObjectModuleAccessor;
    let (x, y) = (PostureModule::pos_x(me), PostureModule::pos_y(me));
    let mut nearest = None;
    for record in crate::slight::agents::all_records() {
        if record.entry_id == expected.entry_id || record.category != 0 {
            continue;
        }
        let Some(name) = registry::fighter_name(record.kind).filter(|name| *name != "ptrainer")
        else {
            continue;
        };
        let Some(identity) = Identity::from_live_fighter(record.boid, name) else {
            continue;
        };
        let module = identity.module_accessor_addr as *mut smash::app::BattleObjectModuleAccessor;
        let status = StatusModule::status_kind(module);
        if status == *FIGHTER_STATUS_KIND_DEAD || status == *FIGHTER_STATUS_KIND_STANDBY {
            continue;
        }
        let (dx, dy) = (
            PostureModule::pos_x(module) - x,
            PostureModule::pos_y(module) - y,
        );
        let distance = dx * dx + dy * dy;
        if match nearest {
            Some((best, _)) => distance < best,
            None => true,
        } {
            nearest = Some((distance, module));
        }
    }
    nearest.map(|(_, module)| module)
}

/// Every opponent reading; absence is a defined value, never a batch failure.
unsafe fn sample_opponent(expected: Identity, value: FighterValue) -> NativeObservation {
    let me = expected.module_accessor_addr as *mut smash::app::BattleObjectModuleAccessor;
    let opponent = nearest_opponent(expected);
    if value == FighterValue::OpponentPresent {
        return NativeObservation::Flag(opponent.is_some());
    }
    let Some(opponent) = opponent else {
        return NativeObservation::Float(0.0);
    };
    NativeObservation::Float(match value {
        FighterValue::OpponentDamage => {
            let damage = DamageModule::damage(opponent, 0);
            // One line per whole percent: this path runs every frame.
            static LOGGED: AtomicI32 = AtomicI32::new(-1);
            if LOGGED.swap(damage as i32, Ordering::Relaxed) != damage as i32 {
                crate::slight::diag::note(format!("opponent damage {damage:.1}"));
            }
            damage
        }
        FighterValue::OpponentDx => PostureModule::pos_x(opponent) - PostureModule::pos_x(me),
        FighterValue::OpponentDxForward => {
            (PostureModule::pos_x(opponent) - PostureModule::pos_x(me)) * PostureModule::lr(me)
        }
        _ => PostureModule::pos_y(opponent) - PostureModule::pos_y(me),
    })
}

pub(crate) const MAX_WORK_SAMPLES: usize = 64;

#[derive(Debug, Clone, Copy)]
pub(crate) struct WorkSource<'a> {
    pub symbol: &'a str,
    pub kind: registry::IdentifierKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum WorkValue {
    Float(f32),
    Int(i32),
    Flag(bool),
}

/// Registry IDs resolved for the whole batch before any read. Every other
/// source reads a closed engine API directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedNativeId {
    Direct,
    Work(i32),
    Article(i32),
    FighterStatus(i32),
}

/// Read all declared behavior inputs as one dispatch-local snapshot. Source
/// eligibility is narrower than registry membership and stays closed here.
pub(crate) fn sample_behavior_inputs(
    boid: u32,
    fighter: &str,
    expected: Identity,
    sources: &[NativeInput],
) -> Result<NativeSnapshot, String> {
    sample_behavior_inputs_guarded(boid, fighter, expected, sources, || Ok(()))
}

pub(crate) fn sample_behavior_inputs_guarded(
    boid: u32,
    fighter: &str,
    expected: Identity,
    sources: &[NativeInput],
    mut check_context: impl FnMut() -> Result<(), String>,
) -> Result<NativeSnapshot, String> {
    if sources.len() > MAX_WORK_SAMPLES {
        return Err(format!("native inputs exceed {MAX_WORK_SAMPLES} sources"));
    }
    let mut seen = BTreeSet::new();
    let mut canonical_sources = Vec::with_capacity(sources.len());
    for source in sources {
        if !source.valid_for(fighter) {
            return Err(format!(
                "native input {:?} is unsupported for fighter {fighter:?}",
                source
            ));
        }
        let canonical = source.canonical();
        if !seen.insert(canonical.clone()) {
            continue;
        }
        if canonical_sources.len() >= MAX_WORK_SAMPLES {
            return Err(format!("native inputs exceed {MAX_WORK_SAMPLES} sources"));
        }
        canonical_sources.push(canonical);
    }
    // Resolve the complete batch before reading anything. Damage, attack-result
    // and article inputs use closed read APIs, not Work IDs; ordinary native values
    // retain the exact registry-backed Work path.
    let mut native_ids = Vec::with_capacity(canonical_sources.len());
    for source in &canonical_sources {
        if matches!(
            source,
            NativeInput::ParamFloat { .. }
                | NativeInput::ParamInt { .. }
                | NativeInput::EnergySpeed { .. }
                | NativeInput::Fighter { .. }
                | NativeInput::OwnedObject { .. }
                | NativeInput::Command { .. }
                | NativeInput::AttackResultFlag { .. }
                | NativeInput::HeroMenuSpell
        ) {
            native_ids.push(ResolvedNativeId::Direct);
            continue;
        }
        if let NativeInput::FighterStatusIs { fighter, symbol } = source {
            let identifier =
                registry::resolve(fighter, symbol, registry::IdentifierKind::FighterStatus)?;
            if !matches!(identifier.group, "" | "common") {
                return Err(format!(
                    "status {symbol:?} is outside its fighter status groups"
                ));
            }
            native_ids.push(ResolvedNativeId::FighterStatus(
                i32::try_from(identifier.value)
                    .map_err(|_| format!("status {symbol:?} does not fit its native ID"))?,
            ));
            continue;
        }
        if let NativeInput::ArticleExists { fighter, symbol }
        | NativeInput::ArticleCount { fighter, symbol }
        | NativeInput::ArticleDeflected {
            fighter, symbol, ..
        } = source
        {
            let identifier = registry::resolve(fighter, symbol, registry::IdentifierKind::Article)?;
            if !identifier.group.is_empty() {
                return Err(format!("article {symbol:?} is not fighter-owned"));
            }
            native_ids.push(ResolvedNativeId::Article(
                i32::try_from(identifier.value)
                    .map_err(|_| format!("article {symbol:?} does not fit its native ID"))?,
            ));
            continue;
        }
        let kind = match source.kind() {
            Some(NativeKind::Float) => registry::IdentifierKind::WorkFloat,
            Some(NativeKind::Int) => registry::IdentifierKind::WorkInt,
            Some(NativeKind::Flag) => registry::IdentifierKind::WorkFlag,
            None => unreachable!("unsupported source rejected above"),
        };
        let id = registry::work_value_id(fighter, source.symbol(), kind)?;
        native_ids.push(ResolvedNativeId::Work(i32::try_from(id).map_err(|_| {
            format!(
                "native value {:?} does not fit its native ID",
                source.symbol()
            )
        })?));
    }
    let accessor = expected.module_accessor_addr as *mut smash::app::BattleObjectModuleAccessor;
    // Every status condition in this immutable dispatch sees the same sampled
    // status, even when several different symbolic comparisons are requested.
    let mut sampled_status = None;
    let observations = sample_ordered_native_inputs(
        &canonical_sources,
        || {
            require_identity(boid, fighter, expected)?;
            check_context()
        },
        |index, source| {
            let native_id = native_ids[index];
            let observation = unsafe {
                match source {
                    NativeInput::ParamFloat { table, key, .. } => {
                        NativeObservation::Float(WorkModule::get_param_float(
                            accessor,
                            super::native_inputs::hash40(table),
                            super::native_inputs::hash40(key),
                        ))
                    }
                    NativeInput::ParamInt { table, key, .. } => {
                        NativeObservation::Int(WorkModule::get_param_int(
                            accessor,
                            super::native_inputs::hash40(table),
                            super::native_inputs::hash40(key),
                        ))
                    }
                    NativeInput::EnergySpeed { energy, axis, .. } => {
                        let ptr = KineticModule::get_energy(
                            accessor,
                            super::status_runtime::energy_id(*energy),
                        ) as *mut smash::app::KineticEnergy;
                        if ptr.is_null() {
                            return Err("kinetic energy is unavailable".into());
                        }
                        NativeObservation::Float(match axis {
                            super::native_inputs::Axis::X => {
                                smash::app::lua_bind::KineticEnergy::get_speed_x(ptr)
                            }
                            super::native_inputs::Axis::Y => {
                                smash::app::lua_bind::KineticEnergy::get_speed_y(ptr)
                            }
                        })
                    }
                    NativeInput::Fighter { value, .. } => {
                        use FighterValue as V;
                        let main = *smash::lib::lua_const::KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN;
                        match value {
                            V::StatusChanging => {
                                NativeObservation::Flag(StatusModule::is_changing(accessor))
                            }
                            V::PartnerPresent | V::PartnerDamage => {
                                super::partner_control::sample(boid, fighter, expected, source)?
                            }
                            V::HoldingOpponent
                            | V::HeldOpponentDamage
                            | V::HeldOpponentWeight
                            | V::HeldOpponentEscapeTime => {
                                super::grab_control::sample(boid, fighter, expected, source)?
                            }
                            V::SpeedX => NativeObservation::Float(KineticModule::get_sum_speed_x(
                                accessor, main,
                            )),
                            V::SpeedY => NativeObservation::Float(KineticModule::get_sum_speed_y(
                                accessor, main,
                            )),
                            V::PositionX => {
                                NativeObservation::Float(PostureModule::pos_x(accessor))
                            }
                            V::PositionY => {
                                NativeObservation::Float(PostureModule::pos_y(accessor))
                            }
                            V::Facing => NativeObservation::Float(PostureModule::lr(accessor)),
                            V::Damage => {
                                NativeObservation::Float(DamageModule::damage(accessor, 0))
                            }
                            V::StocksRemaining | V::OpponentStocksRemaining => {
                                NativeObservation::Int(sample_stocks(
                                    expected,
                                    *value == V::OpponentStocksRemaining,
                                )?)
                            }
                            V::OpponentPresent
                            | V::OpponentDamage
                            | V::OpponentDx
                            | V::OpponentDxForward
                            | V::OpponentDy => sample_opponent(expected, *value),
                            V::HeldItemKind => {
                                NativeObservation::Int(ItemModule::get_have_item_kind(accessor, 0))
                            }
                            V::HoldingItem => {
                                NativeObservation::Flag(ItemModule::is_have_item(accessor, 0))
                            }
                        }
                    }
                    NativeInput::HeroMenuSpell => NativeObservation::Int(
                        super::hero_menu_control::sample(boid, fighter, expected)?,
                    ),
                    NativeInput::OwnedObject {
                        fighter,
                        symbol,
                        field,
                    } => {
                        // Resolve the accessor afresh for every sample, behind the
                        // active-object fence; absence reads as zero / not alive.
                        use super::native_inputs::ObjectField as F;
                        use smash::app::sv_battle_object as object;
                        let module =
                            super::article_context::newest_owned_article(boid, fighter, symbol)
                                .filter(|id| !object::is_null(*id) && object::is_active(*id))
                                .map(|id| object::module_accessor(id))
                                .filter(|module| !module.is_null());
                        let main = *smash::lib::lua_const::KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN;
                        match (field, module) {
                            (F::Alive, module) => NativeObservation::Flag(module.is_some()),
                            (_, None) => NativeObservation::Float(0.0),
                            (F::PosX, Some(m)) => NativeObservation::Float(PostureModule::pos_x(m)),
                            (F::PosY, Some(m)) => NativeObservation::Float(PostureModule::pos_y(m)),
                            (F::SpeedX, Some(m)) => {
                                NativeObservation::Float(KineticModule::get_sum_speed_x(m, main))
                            }
                            (F::SpeedY, Some(m)) => {
                                NativeObservation::Float(KineticModule::get_sum_speed_y(m, main))
                            }
                        }
                    }
                    NativeInput::FighterStatusIs { .. } => {
                        let expected_status = match native_id {
                            ResolvedNativeId::FighterStatus(id) => id,
                            _ => unreachable!("status source was pre-resolved"),
                        };
                        let status = *sampled_status
                            .get_or_insert_with(|| StatusModule::status_kind(accessor));
                        if status < 0 {
                            return Err("native current status is unavailable".into());
                        }
                        NativeObservation::Flag(status == expected_status)
                    }
                    NativeInput::Command { symbol, .. } => {
                        use smash::lib::lua_const::*;
                        let mask = match symbol.as_str() {
                            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N_COMMAND" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N_COMMAND
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N2_COMMAND" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N2_COMMAND
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_S_COMMAND" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_S_COMMAND
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_HI_COMMAND" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_HI_COMMAND
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_HI2_COMMAND" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_HI2_COMMAND
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_SUPER_SPECIAL_COMMAND" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_SUPER_SPECIAL_COMMAND
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_SUPER_SPECIAL2_COMMAND" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_SUPER_SPECIAL2_COMMAND
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_6N6" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_6N6
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_4N4" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_4N4
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623NB" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623NB
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623A" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623A
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_ATTACK_COMMAND1" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_ATTACK_COMMAND1
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623STRICT" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623STRICT
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623ALONG" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623ALONG
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623BLONG" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_623BLONG
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_6N6AB" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_6N6AB
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_323CATCH" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_323CATCH
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_1" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_1
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_3" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_3
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_7" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_7
                            }
                            "FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_9" => {
                                *FIGHTER_PAD_CMD_CAT4_FLAG_COMMAND_9
                            }
                            _ => return Err("unsupported command flag".into()),
                        };
                        NativeObservation::Flag(
                            smash::app::lua_bind::ControlModule::get_command_flag_cat(accessor, 3)
                                & mask
                                != 0,
                        )
                    }
                    NativeInput::AttackResultFlag { query, mask, .. } => {
                        let mask = match mask {
                            AttackResultMask::Hit => *COLLISION_KIND_MASK_HIT,
                            AttackResultMask::Shield => *COLLISION_KIND_MASK_SHIELD,
                        };
                        let value = match query {
                            AttackResultQuery::DuringHitlag => {
                                AttackModule::is_infliction(accessor, mask)
                            }
                            AttackResultQuery::CurrentStatus => {
                                AttackModule::is_infliction_status(accessor, mask)
                            }
                        };
                        NativeObservation::Flag(value)
                    }
                    NativeInput::ArticleCount { .. } => {
                        let ResolvedNativeId::Article(id) = native_id else {
                            unreachable!("article source pre-resolved")
                        };
                        NativeObservation::Int(ArticleModule::get_active_num(accessor, id))
                    }
                    NativeInput::ArticleDeflected {
                        fighter,
                        symbol,
                        absorbed,
                    } => {
                        let mask = if *absorbed {
                            *COLLISION_KIND_MASK_ABSORBER
                        } else {
                            *COLLISION_KIND_MASK_REFLECTOR
                        };
                        let mut hit = false;
                        for weapon in crate::slight::systems::article_notifier::owned_weapons(
                            boid, fighter, symbol,
                        ) {
                            if smash::app::sv_battle_object::is_null(weapon)
                                || !smash::app::sv_battle_object::is_active(weapon)
                            {
                                continue;
                            }
                            let weapon = smash::app::sv_battle_object::module_accessor(weapon);
                            hit |= !weapon.is_null()
                                && AttackModule::is_infliction_status(weapon, mask);
                        }
                        NativeObservation::Flag(hit)
                    }
                    NativeInput::ArticleExists { .. } => {
                        let article_id = match native_id {
                            ResolvedNativeId::Article(id) => id,
                            _ => unreachable!("article source was pre-resolved"),
                        };
                        NativeObservation::Flag(ArticleModule::is_exist(accessor, article_id))
                    }
                    _ => {
                        let id = match native_id {
                            ResolvedNativeId::Work(id) => id,
                            _ => unreachable!("Work source was pre-resolved"),
                        };
                        let projectile = source
                            .projectile_article()
                            .map(|article| newest_projectile(boid, fighter, article));
                        let accessor = match projectile {
                            Some(Some(weapon)) => {
                                smash::app::sv_battle_object::module_accessor(weapon)
                            }
                            Some(None) => {
                                return Ok(match source.kind() {
                                    Some(NativeKind::Float) => NativeObservation::Float(0.0),
                                    Some(NativeKind::Int) => NativeObservation::Int(0),
                                    _ => NativeObservation::Flag(false),
                                })
                            }
                            None => accessor,
                        };
                        if accessor.is_null() {
                            return Err("projectile accessor unavailable".into());
                        }
                        match source.kind() {
                            Some(NativeKind::Float) => {
                                NativeObservation::Float(WorkModule::get_float(accessor, id))
                            }
                            Some(NativeKind::Int) => {
                                NativeObservation::Int(WorkModule::get_int(accessor, id))
                            }
                            Some(NativeKind::Flag) => {
                                NativeObservation::Flag(WorkModule::is_flag(accessor, id))
                            }
                            None => unreachable!("unsupported source rejected before native reads"),
                        }
                    }
                }
            };
            if matches!(observation, NativeObservation::Float(value) if !value.is_finite()) {
                return Err(format!("native value {:?} is not finite", source.symbol()));
            }
            if matches!(observation, NativeObservation::Int(value) if i64::from(value).abs() > super::native_inputs::MAX_ABS_NATIVE_VALUE as i64)
            {
                return Err(format!(
                    "native value {:?} is outside the live numeric range",
                    source.symbol()
                ));
            }
            Ok(observation)
        },
    )?;
    let mut snapshot = NativeSnapshot::from_observations(&canonical_sources, &observations)?;
    snapshot.article_event = super::article_context::sample(boid, expected);
    Ok(snapshot)
}

/// All-or-none ordered sampling core. Production supplies live-identity checks
/// and engine getters; tests inject deterministic readers without fake engine
/// pointers. No partially collected vector escapes on any failure.
fn sample_ordered_native_inputs(
    sources: &[NativeInput],
    mut check_identity: impl FnMut() -> Result<(), String>,
    mut read: impl FnMut(usize, &NativeInput) -> Result<NativeObservation, String>,
) -> Result<Vec<NativeObservation>, String> {
    if sources.len() > MAX_WORK_SAMPLES {
        return Err(format!("native inputs exceed {MAX_WORK_SAMPLES} sources"));
    }
    check_identity()?;
    let mut observations = Vec::with_capacity(sources.len());
    for (index, source) in sources.iter().enumerate() {
        check_identity()?;
        let value = read(index, source)?;
        check_identity()?;
        observations.push(value);
    }
    Ok(observations)
}

/// Read static adapter declarations through the same reviewed mapping.
pub(crate) fn sample_static_inputs(
    boid: u32,
    fighter: &str,
    expected: Identity,
    sources: &[NativeSource],
) -> Result<NativeSnapshot, String> {
    sample_static_inputs_guarded(boid, fighter, expected, sources, || Ok(()))
}

pub(crate) fn sample_static_inputs_guarded(
    boid: u32,
    fighter: &str,
    expected: Identity,
    sources: &[NativeSource],
    check_context: impl FnMut() -> Result<(), String>,
) -> Result<NativeSnapshot, String> {
    if sources.len() > MAX_WORK_SAMPLES {
        return Err(format!(
            "static native inputs exceed {MAX_WORK_SAMPLES} sources"
        ));
    }
    let sources = sources
        .iter()
        .map(|source| (*source).to_input())
        .collect::<Vec<_>>();
    sample_behavior_inputs_guarded(boid, fighter, expected, &sources, check_context)
}

fn require_identity(boid: u32, fighter: &str, expected: Identity) -> Result<(), String> {
    if Identity::from_live_fighter(boid, fighter) != Some(expected) {
        return Err(format!(
            "native values for {fighter:?}, actor {boid:#x}: fighter/entry/accessor/stock identity changed"
        ));
    }
    Ok(())
}

/// Sample each source once in declaration order, or return no snapshot.
///
/// All identifiers are checked before the first engine read. Ownership is
/// rechecked immediately before and after every getter. The returned values
/// are local to this evaluation; never copy them into the controller's mutable
/// variable store or substitute a fabricated zero for an error.
pub(crate) fn sample_work_values(
    boid: u32,
    fighter: &str,
    expected: Identity,
    sources: &[WorkSource<'_>],
) -> Result<Vec<WorkValue>, String> {
    if sources.len() > MAX_WORK_SAMPLES {
        return Err(format!("native values exceed {MAX_WORK_SAMPLES} sources"));
    }
    let mut names = BTreeSet::new();
    let mut identifiers = Vec::with_capacity(sources.len());
    for source in sources {
        if !matches!(
            source.kind,
            registry::IdentifierKind::WorkFloat
                | registry::IdentifierKind::WorkInt
                | registry::IdentifierKind::WorkFlag
        ) {
            return Err(format!(
                "native value {:?} is not a float, integer or flag Work source",
                source.symbol
            ));
        }
        if !names.insert(source.symbol) {
            return Err(format!("duplicate native value source {:?}", source.symbol));
        }
        let identifier = registry::work_value_id(fighter, source.symbol, source.kind)?;
        let id = i32::try_from(identifier).map_err(|_| {
            format!(
                "native value {:?} does not fit its native ID",
                source.symbol
            )
        })?;
        identifiers.push(id);
    }
    require_identity(boid, fighter, expected)?;
    let mut values = Vec::with_capacity(sources.len());
    for (source, id) in sources.iter().zip(identifiers) {
        require_identity(boid, fighter, expected)?;
        // The preceding exact identity check proves this accessor belongs to
        // the current fighter, not an old object sharing its BOID or entry.
        let projectile = registry::projectile_work(fighter, source.symbol, source.kind);
        let accessor = if let Some((_, article)) = projectile {
            match newest_projectile(boid, fighter, article.symbol) {
                Some(weapon) => unsafe { smash::app::sv_battle_object::module_accessor(weapon) },
                None => {
                    values.push(match source.kind {
                        registry::IdentifierKind::WorkFloat => WorkValue::Float(0.0),
                        registry::IdentifierKind::WorkInt => WorkValue::Int(0),
                        _ => WorkValue::Flag(false),
                    });
                    continue;
                }
            }
        } else {
            expected.module_accessor_addr as *mut smash::app::BattleObjectModuleAccessor
        };
        if accessor.is_null() {
            return Err("projectile accessor unavailable".into());
        }
        let value = unsafe {
            match source.kind {
                registry::IdentifierKind::WorkFloat => {
                    WorkValue::Float(WorkModule::get_float(accessor, id))
                }
                registry::IdentifierKind::WorkInt => {
                    WorkValue::Int(WorkModule::get_int(accessor, id))
                }
                registry::IdentifierKind::WorkFlag => {
                    WorkValue::Flag(WorkModule::is_flag(accessor, id))
                }
                _ => unreachable!("source kind was checked before native reads"),
            }
        };
        require_identity(boid, fighter, expected)?;
        if matches!(value, WorkValue::Float(value) if !value.is_finite()) {
            return Err(format!("native value {:?} is not finite", source.symbol));
        }
        if matches!(value, WorkValue::Int(value) if i64::from(value).abs() > super::native_inputs::MAX_ABS_NATIVE_VALUE as i64)
        {
            return Err(format!(
                "native value {:?} is outside the live numeric range",
                source.symbol
            ));
        }
        values.push(value);
    }
    Ok(values)
}

pub(crate) fn sample_controller_input(
    boid: u32,
    fighter: &str,
    expected: Identity,
) -> Result<super::native_inputs::ControllerInput, String> {
    use super::native_inputs::{ButtonEdge, ControllerInput, InputButton};
    use smash::app::lua_bind::{ControlModule, PostureModule};
    use smash::lib::lua_const::*;
    require_identity(boid, fighter, expected)?;
    let module = expected.module_accessor_addr as *mut smash::app::BattleObjectModuleAccessor;
    let mut input = ControllerInput::default();
    for (button, id) in [
        (InputButton::Attack, *CONTROL_PAD_BUTTON_ATTACK),
        (InputButton::Special, *CONTROL_PAD_BUTTON_SPECIAL),
        (InputButton::Jump, *CONTROL_PAD_BUTTON_JUMP),
        (InputButton::Guard, *CONTROL_PAD_BUTTON_GUARD),
        (InputButton::Grab, *CONTROL_PAD_BUTTON_CATCH),
    ] {
        for edge in [ButtonEdge::Held, ButtonEdge::Pressed, ButtonEdge::Released] {
            require_identity(boid, fighter, expected)?;
            let value = unsafe {
                match edge {
                    ButtonEdge::Held => ControlModule::check_button_on(module, id),
                    ButtonEdge::Pressed => ControlModule::check_button_trigger(module, id),
                    ButtonEdge::Released => ControlModule::check_button_release(module, id),
                }
            };
            require_identity(boid, fighter, expected)?;
            input.buttons.insert((button, edge), value);
        }
    }
    input.stick_x = unsafe { ControlModule::get_stick_x(module) };
    require_identity(boid, fighter, expected)?;
    input.stick_y = unsafe { ControlModule::get_stick_y(module) };
    require_identity(boid, fighter, expected)?;
    input.facing = unsafe { PostureModule::lr(module) };
    require_identity(boid, fighter, expected)?;
    if !(-1.0..=1.0).contains(&input.stick_x)
        || !(-1.0..=1.0).contains(&input.stick_y)
        || !matches!(input.facing, -1.0 | 1.0)
    {
        return Err("controller input axes or facing are invalid".into());
    }
    Ok(input)
}

pub(crate) fn preflight_write(
    boid: u32,
    fighter: &str,
    expected: Identity,
    action: &super::live_model::Action,
) -> Result<(), String> {
    use super::live_model::{Action, CmpOp, Expr, Predicate, Value};
    let (input, kind) = match action {
        Action::SetNative { input, value } | Action::AddNative { input, value } => {
            let kind = match value {
                Expr::Const(Value::Float(v))
                    if v.is_finite()
                        && f64::from(*v).abs() <= super::native_inputs::MAX_ABS_NATIVE_VALUE =>
                {
                    NativeKind::Float
                }
                Expr::Const(Value::Int(v))
                    if i64::from(*v).abs() <= super::native_inputs::MAX_ABS_NATIVE_VALUE as i64 =>
                {
                    NativeKind::Int
                }
                _ => return Err("native numeric write was not resolved to a bounded value".into()),
            };
            (input, kind)
        }
        Action::SetNativeFlag {
            input,
            value:
                Predicate::Compare {
                    op: CmpOp::Eq,
                    left: Expr::Const(Value::Int(0 | 1)),
                    right: Expr::Const(Value::Int(1)),
                },
        } => (input, NativeKind::Flag),
        Action::SetNativeFlag { .. } => {
            return Err("native flag write was not resolved to a boolean".into())
        }
        _ => return Ok(()),
    };
    input.validate_write(fighter)?;
    if input.kind() != Some(kind) {
        return Err("native write kind mismatch".into());
    }
    require_identity(boid, fighter, expected)
}

pub(crate) fn execute_write(
    boid: u32,
    fighter: &str,
    expected: Identity,
    action: &super::live_model::Action,
) -> Result<(), String> {
    use super::live_model::{Action, Expr, Predicate, Value};
    preflight_write(boid, fighter, expected, action)?;
    let (input, add) = match action {
        Action::SetNative { input, .. } | Action::SetNativeFlag { input, .. } => (input, false),
        Action::AddNative { input, .. } => (input, true),
        _ => return Ok(()),
    };
    let kind = match input.kind() {
        Some(NativeKind::Float) => registry::IdentifierKind::WorkFloat,
        Some(NativeKind::Int) => registry::IdentifierKind::WorkInt,
        _ => registry::IdentifierKind::WorkFlag,
    };
    let id = registry::work_value_id(fighter, input.symbol(), kind)? as i32;
    let modules = if let Some(article) = input.projectile_article() {
        crate::slight::systems::article_notifier::owned_weapons(boid, fighter, article)
            .into_iter()
            .map(|weapon| unsafe { smash::app::sv_battle_object::module_accessor(weapon) })
            .collect::<Vec<_>>()
    } else {
        vec![expected.module_accessor_addr as *mut smash::app::BattleObjectModuleAccessor]
    };
    for module in modules {
        if module.is_null() {
            return Err("projectile accessor unavailable".into());
        }
        require_identity(boid, fighter, expected)?;
        unsafe {
            match action {
                Action::SetNative {
                    value: Expr::Const(Value::Float(v)),
                    ..
                }
                | Action::AddNative {
                    value: Expr::Const(Value::Float(v)),
                    ..
                } => {
                    let next = if add {
                        WorkModule::get_float(module, id) + v
                    } else {
                        *v
                    };
                    if !next.is_finite()
                        || f64::from(next).abs() > super::native_inputs::MAX_ABS_NATIVE_VALUE
                    {
                        return Err("native float add overflow".into());
                    }
                    require_identity(boid, fighter, expected)?;
                    WorkModule::set_float(module, next, id);
                }
                Action::SetNative {
                    value: Expr::Const(Value::Int(v)),
                    ..
                }
                | Action::AddNative {
                    value: Expr::Const(Value::Int(v)),
                    ..
                } => {
                    let next = if add {
                        WorkModule::get_int(module, id)
                            .checked_add(*v)
                            .ok_or("native integer add overflow")?
                    } else {
                        *v
                    };
                    if i64::from(next).abs() > super::native_inputs::MAX_ABS_NATIVE_VALUE as i64 {
                        return Err("native integer add out of range".into());
                    }
                    let mut next = next;
                    if let Some((common, (min, max))) =
                        super::native_inputs::common_work(input.symbol())
                            .and_then(|common| Some((common, common.write?)))
                    {
                        next = next.clamp(min, max);
                        let current = WorkModule::get_int(module, id);
                        if next == current {
                            continue;
                        }
                        crate::slight::diag::note(format!(
                            "common work {fighter} {}: {current} -> {next}",
                            common.label
                        ));
                    }
                    require_identity(boid, fighter, expected)?;
                    WorkModule::set_int(module, next, id);
                }
                Action::SetNativeFlag {
                    value:
                        Predicate::Compare {
                            left: Expr::Const(Value::Int(v)),
                            ..
                        },
                    ..
                } => {
                    if *v == 1 {
                        WorkModule::on_flag(module, id)
                    } else {
                        WorkModule::off_flag(module, id)
                    }
                }
                _ => return Err("native write unresolved".into()),
            }
        }
    }
    require_identity(boid, fighter, expected)
}

/// Use the same validated objects as movement; ArticleModule instance getters
/// return null for live fireballs. Registration order follows object lifetimes.
fn newest_projectile(owner: u32, fighter: &str, symbol: &str) -> Option<u32> {
    super::article_context::newest_owned_article(owner, fighter, symbol)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(query: AttackResultQuery, mask: AttackResultMask) -> NativeInput {
        NativeInput::AttackResultFlag {
            fighter: "mario".into(),
            query,
            mask,
        }
    }

    #[test]
    fn ordered_snapshot_reader_samples_once_and_checks_identity_around_each_read() {
        let sources = [
            flag(AttackResultQuery::DuringHitlag, AttackResultMask::Hit),
            flag(AttackResultQuery::CurrentStatus, AttackResultMask::Shield),
        ];
        let mut identity_checks = 0;
        let mut reads = Vec::new();
        let observations = sample_ordered_native_inputs(
            &sources,
            || {
                identity_checks += 1;
                Ok(())
            },
            |index, source| {
                reads.push((index, source.clone()));
                Ok(NativeObservation::Flag(index == 0))
            },
        )
        .unwrap();
        assert_eq!(identity_checks, 1 + sources.len() * 2);
        assert_eq!(
            reads,
            vec![(0, sources[0].clone()), (1, sources[1].clone())]
        );
        assert_eq!(
            observations,
            [
                NativeObservation::Flag(true),
                NativeObservation::Flag(false)
            ]
        );
    }

    #[test]
    fn identity_change_or_getter_error_returns_no_partial_batch() {
        let sources = [
            flag(AttackResultQuery::DuringHitlag, AttackResultMask::Hit),
            flag(AttackResultQuery::CurrentStatus, AttackResultMask::Shield),
        ];
        let mut checks = 0;
        let mut reads = 0;
        let identity_changed = sample_ordered_native_inputs(
            &sources,
            || {
                checks += 1;
                if checks == 3 {
                    Err("identity changed".into())
                } else {
                    Ok(())
                }
            },
            |_, _| {
                reads += 1;
                Ok(NativeObservation::Flag(true))
            },
        );
        assert!(identity_changed.is_err());
        assert_eq!(reads, 1);

        let error = sample_ordered_native_inputs(
            &sources,
            || Ok(()),
            |index, _| {
                if index == 1 {
                    Err("getter failed".into())
                } else {
                    Ok(NativeObservation::Flag(true))
                }
            },
        );
        assert!(error.is_err());
    }
}
