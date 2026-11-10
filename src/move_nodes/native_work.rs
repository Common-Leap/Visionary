//! Closed source-only admission for WorkModule operations outside the live registry.
//!
//! Common flags, exact per-owner status flags and transition terms,
//! and measured ItemGrassPull setters are admitted only for typed ACMD
//! reconstruction. They do not enter the shared gimmick registry or authorize
//! live native writes.

use crate::gimmicks::registry::{self, IdentifierKind};

pub const DASH_TO_RUN_TERM: &str = "*FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN";

/// Everyday ways out of a move, shared by every fighter.
const COMMON_CANCELS: &[(&str, &str)] = &[
    (
        "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_JUMP_SQUAT_BUTTON",
        "Jump (button)",
    ),
    (
        "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_JUMP_SQUAT",
        "Jump (stick up)",
    ),
    (
        "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_JUMP_AERIAL_BUTTON",
        "Air jump (button)",
    ),
    (
        "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_JUMP_AERIAL",
        "Air jump (stick up)",
    ),
    ("*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK", "Jab"),
    (
        "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_AIR",
        "Air attack",
    ),
    (
        "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_N",
        "Neutral special",
    ),
    (
        "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_S",
        "Side special",
    ),
    (
        "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_HI",
        "Up special",
    ),
    (
        "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_LW",
        "Down special",
    ),
    ("*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_GUARD_ON", "Shield"),
    ("*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ESCAPE", "Dodge"),
];

/// The plain name of a cancel, whichever fighter or action it came from.
pub fn transition_term_label(term: &str) -> Option<&'static str> {
    COMMON_CANCELS
        .iter()
        .find(|(known, _)| *known == term)
        .map(|(_, label)| *label)
        .or_else(|| {
            source_transition_term_descriptors()
                .iter()
                .find(|descriptor| descriptor.transition_term == term)
                .map(|descriptor| descriptor.label)
        })
}

/// A measured source-only transition-term operation admitted for one concrete fighter.
/// The symbolic token remains part of the identity: several owner-specific constants share
/// the same numeric Work ID in the pinned registry.
#[derive(Debug, Clone, Copy)]
pub struct SourceTransitionTermDescriptor<'a> {
    pub fighter: &'a str,
    pub action: crate::data::WorkTransitionTermAction,
    pub transition_term: &'static str,
    pub label: &'static str,
}

const MEASURED_SOURCE_TRANSITION_TERMS: &[SourceTransitionTermDescriptor<'static>] = &[
    SourceTransitionTermDescriptor {
        fighter: "buddy",
        action: crate::data::WorkTransitionTermAction::Enable,
        transition_term: "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ITEM_THROW",
        label: "Item throw",
    },
    SourceTransitionTermDescriptor {
        fighter: "daisy",
        action: crate::data::WorkTransitionTermAction::Enable,
        transition_term: "*FIGHTER_PEACH_STATUS_SPECIAL_S_JUMP_ID_TIME_OUT",
        label: "Special-S jump timeout",
    },
    SourceTransitionTermDescriptor {
        fighter: "peach",
        action: crate::data::WorkTransitionTermAction::Enable,
        transition_term: "*FIGHTER_PEACH_STATUS_SPECIAL_S_JUMP_ID_TIME_OUT",
        label: "Special-S jump timeout",
    },
    SourceTransitionTermDescriptor {
        fighter: "koopag",
        action: crate::data::WorkTransitionTermAction::Unable,
        transition_term: DASH_TO_RUN_TERM,
        label: "Dash-to-run unavailable",
    },
    SourceTransitionTermDescriptor {
        fighter: "miienemyf",
        action: crate::data::WorkTransitionTermAction::Unable,
        transition_term: DASH_TO_RUN_TERM,
        label: "Dash-to-run unavailable",
    },
    SourceTransitionTermDescriptor {
        fighter: "miienemyg",
        action: crate::data::WorkTransitionTermAction::Unable,
        transition_term: DASH_TO_RUN_TERM,
        label: "Dash-to-run unavailable",
    },
    SourceTransitionTermDescriptor {
        fighter: "miienemys",
        action: crate::data::WorkTransitionTermAction::Unable,
        transition_term: DASH_TO_RUN_TERM,
        label: "Dash-to-run unavailable",
    },
    SourceTransitionTermDescriptor {
        fighter: "yoshi",
        action: crate::data::WorkTransitionTermAction::EnableGroup,
        transition_term: "*FIGHTER_STATUS_TRANSITION_GROUP_CHK_AIR_LANDING",
        label: "Air landing group",
    },
    SourceTransitionTermDescriptor {
        fighter: "captain",
        action: crate::data::WorkTransitionTermAction::Enable,
        transition_term: "*FIGHTER_STATUS_TRANSITION_TERM_ID_LANDING",
        label: "Landing",
    },
    SourceTransitionTermDescriptor {
        fighter: "snake",
        action: crate::data::WorkTransitionTermAction::Enable,
        transition_term: "*FIGHTER_SNAKE_STATUS_CYPHER_HANG_TRANS_ID_CUT_STICK",
        label: "Cypher stick cut",
    },
    SourceTransitionTermDescriptor {
        fighter: "snake",
        action: crate::data::WorkTransitionTermAction::Enable,
        transition_term: "*FIGHTER_SNAKE_STATUS_CYPHER_HANG_TRANS_ID_CUT_TIME_OUT",
        label: "Cypher timeout cut",
    },
    SourceTransitionTermDescriptor {
        fighter: "luigi",
        action: crate::data::WorkTransitionTermAction::Enable,
        transition_term: "*FIGHTER_LUIGI_STATUS_SPECIAL_S_RAM_TRANSITION_TERM_ID_GROUND",
        label: "Special-S ground transition",
    },
    SourceTransitionTermDescriptor {
        fighter: "diddy",
        action: crate::data::WorkTransitionTermAction::Enable,
        transition_term: "*FIGHTER_DIDDY_STATUS_SPECIAL_S_TRANSITION_TERM_ID_GROUND",
        label: "Special-S ground transition",
    },
    SourceTransitionTermDescriptor {
        fighter: "dedede",
        action: crate::data::WorkTransitionTermAction::Enable,
        transition_term: "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_TURN_DASH",
        label: "Run-brake turn dash",
    },
    SourceTransitionTermDescriptor {
        fighter: "dedede",
        action: crate::data::WorkTransitionTermAction::Enable,
        transition_term: "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK",
        label: "Run-brake attack",
    },
    SourceTransitionTermDescriptor {
        fighter: "dedede",
        action: crate::data::WorkTransitionTermAction::Enable,
        transition_term: "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_S3",
        label: "Run-brake side tilt",
    },
    SourceTransitionTermDescriptor {
        fighter: "dedede",
        action: crate::data::WorkTransitionTermAction::Enable,
        transition_term: "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_HI3",
        label: "Run-brake up tilt",
    },
    SourceTransitionTermDescriptor {
        fighter: "dedede",
        action: crate::data::WorkTransitionTermAction::Enable,
        transition_term: "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_LW3",
        label: "Run-brake down tilt",
    },
    SourceTransitionTermDescriptor {
        fighter: "dedede",
        action: crate::data::WorkTransitionTermAction::Enable,
        transition_term: "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_S4_START",
        label: "Run-brake side smash",
    },
    SourceTransitionTermDescriptor {
        fighter: "dedede",
        action: crate::data::WorkTransitionTermAction::Enable,
        transition_term: "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_LW4_START",
        label: "Run-brake down smash",
    },
];

/// Exact measured, concrete-owner transition-term choices for source reconstruction.
/// Shared Dash→Run Enable remains handled separately by its historical validator branch.
pub fn source_transition_term_descriptors() -> &'static [SourceTransitionTermDescriptor<'static>] {
    MEASURED_SOURCE_TRANSITION_TERMS
}

/// Choices for one concrete fighter, including the historical shared Dash→Run Enable tuple.
/// The shared row carries the caller's concrete fighter so selectors never need a wildcard owner.
pub fn source_transition_term_choices(fighter: &str) -> Vec<SourceTransitionTermDescriptor<'_>> {
    if registry::fighter_kind_symbol(fighter).is_none() {
        return Vec::new();
    }
    let mut choices = vec![SourceTransitionTermDescriptor {
        fighter,
        action: crate::data::WorkTransitionTermAction::Enable,
        transition_term: DASH_TO_RUN_TERM,
        label: "Dash-to-run",
    }];
    for action in [
        crate::data::WorkTransitionTermAction::Enable,
        crate::data::WorkTransitionTermAction::Unable,
    ] {
        choices.extend(COMMON_CANCELS.iter().map(|(transition_term, label)| {
            SourceTransitionTermDescriptor {
                fighter,
                action,
                transition_term,
                label,
            }
        }));
    }
    choices.extend(
        source_transition_term_descriptors()
            .iter()
            .copied()
            .filter(|descriptor| descriptor.fighter == fighter),
    );
    choices
}

/// Find one source-only descriptor by its complete owner/action/symbol tuple.
pub fn source_transition_term_descriptor(
    fighter: &str,
    action: crate::data::WorkTransitionTermAction,
    transition_term: &str,
) -> Option<&'static SourceTransitionTermDescriptor<'static>> {
    source_transition_term_descriptors()
        .iter()
        .find(|descriptor| {
            descriptor.fighter == fighter
                && descriptor.action == action
                && descriptor.transition_term == transition_term
        })
}

/// Exact status WorkInt slot and enum values measured in direct Game Execute calls.
pub const ITEM_GRASS_PULL_NEXT_STEP_SLOT: &str =
    "*FIGHTER_STATUS_ITEM_GRASS_PULL_WORK_INT_NEXT_STEP";
pub const ITEM_GRASS_PULL_STEP_VALUES: &[&str] = &[
    "*FIGHTER_ITEM_GRASS_PULL_STEP_PICKUP",
    "*FIGHTER_ITEM_GRASS_PULL_STEP_PULL",
    "*FIGHTER_ITEM_GRASS_PULL_STEP_HOIST",
    "*FIGHTER_ITEM_GRASS_PULL_STEP_END",
];
/// Exact status WorkInt slot and literal measured alongside ItemGrassPull.
pub const THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX_SLOT: &str =
    "*FIGHTER_STATUS_ITEM_GRASS_PULL_WORK_INT_THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX";
pub const THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX_VALUE: &str = "4";

/// A measured source-only WorkModule setter admitted by exact owner, kind, slot and value.
/// These descriptors are shared with selectors; they do not extend the live Work-ID registry.
#[derive(Debug, Clone, Copy)]
pub struct SourceWorkModuleSetDescriptor {
    pub fighter: &'static str,
    pub kind: crate::data::WorkModuleSetKind,
    pub slot: &'static str,
    pub label: &'static str,
    pub values: &'static [&'static str],
}

const MIISWORDSMAN_WAVE_ANGLE_VALUES: &[&str] = &[
    "-30.0", "-25.0", "-20.0", "-15.0", "-10.0", "-5.0", "0.0", "5.0", "10.0", "15.0", "20.0",
    "25.0", "30.0",
];

const EFLAME_OPEN_MOTION_VALUES: &[&str] = &[
    "hash40(\"final_air_attack\") as i64",
    "hash40(\"final_attack\") as i64",
    "hash40(\"special_air_n4\") as i64",
    "hash40(\"special_air_n_hold\") as i64",
    "hash40(\"special_air_n_start\") as i64",
    "hash40(\"special_n\") as i64",
    "hash40(\"special_n1\") as i64",
    "hash40(\"special_n4\") as i64",
    "hash40(\"special_n_hold\") as i64",
    "hash40(\"special_s\") as i64",
    "hash40(\"visual_scene_01\") as i64",
    "hash40(\"visual_scene_02\") as i64",
];

const ELIGHT_OPEN_MOTION_VALUES: &[&str] = &[
    "hash40(\"attack_hi4_hold\") as i64",
    "hash40(\"attack_lw4_hold\") as i64",
    "hash40(\"attack_s4_hold\") as i64",
    "hash40(\"final_air_attack\") as i64",
    "hash40(\"final_attack\") as i64",
    "hash40(\"special_air_hi_jump\") as i64",
    "hash40(\"special_n_hold\") as i64",
    "hash40(\"visual_scene_01\") as i64",
    "hash40(\"visual_scene_02\") as i64",
    "hash40(\"visual_scene_03\") as i64",
    "hash40(\"visual_scene_04\") as i64",
];

const SOURCE_WORK_MODULE_SET_DESCRIPTORS: &[SourceWorkModuleSetDescriptor] = &[
    SourceWorkModuleSetDescriptor {
        fighter: "miiswordsman",
        kind: crate::data::WorkModuleSetKind::Float,
        slot: "*FIGHTER_MIISWORDSMAN_STATUS_FINAL_WORK_ID_FLOAT_WAVE_ANGLE",
        label: "Final wave angle",
        values: MIISWORDSMAN_WAVE_ANGLE_VALUES,
    },
    SourceWorkModuleSetDescriptor {
        fighter: "eflame",
        kind: crate::data::WorkModuleSetKind::Int64,
        slot: "*FIGHTER_ELIGHT_INSTANCE_WORK_ID_INT_ESWORD_INHERIT_OPEN_MOTION_KIND",
        label: "Inherited open motion",
        values: EFLAME_OPEN_MOTION_VALUES,
    },
    SourceWorkModuleSetDescriptor {
        fighter: "elight",
        kind: crate::data::WorkModuleSetKind::Int64,
        slot: "*FIGHTER_ELIGHT_INSTANCE_WORK_ID_INT_ESWORD_INHERIT_OPEN_MOTION_KIND",
        label: "Inherited open motion",
        values: ELIGHT_OPEN_MOTION_VALUES,
    },
];

/// List the bounded measured setters for selector reuse.
pub fn source_work_module_set_descriptors() -> &'static [SourceWorkModuleSetDescriptor] {
    SOURCE_WORK_MODULE_SET_DESCRIPTORS
}

/// Look up one exact, closed source-only setter descriptor for UI and validation reuse.
pub fn source_work_module_set_descriptor(
    fighter: &str,
    kind: crate::data::WorkModuleSetKind,
    slot: &str,
) -> Option<&'static SourceWorkModuleSetDescriptor> {
    source_work_module_set_descriptors()
        .iter()
        .find(|descriptor| {
            descriptor.fighter == fighter && descriptor.kind == kind && descriptor.slot == slot
        })
}

const SOURCE_NUMERIC_WORK_SLOTS: &[(&str, crate::data::WorkModuleSetKind, &str)] = &[
    (
        "gamewatch",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_GAMEWATCH_STATUS_THROW_WORK_FLOAT_STOCK_ICON_ROTATE",
    ),
    (
        "yoshi",
        crate::data::WorkModuleSetKind::Int,
        "*FIGHTER_YOSHI_STATUS_FINAL_WORK_INT_CHANGE_MOTION_TARGET",
    ),
    (
        "littlemac",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_LITTLEMAC_STATUS_SPECIAL_N_FLOAT_POWER_MIN",
    ),
    (
        "littlemac",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_LITTLEMAC_STATUS_SPECIAL_N_FLOAT_POWER_MAX",
    ),
    (
        "trail",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_TRAIL_STATUS_SPECIAL_N2_FLOAT_ANGLE",
    ),
    (
        "trail",
        crate::data::WorkModuleSetKind::Int,
        "*FIGHTER_TRAIL_STATUS_SPECIAL_N3_INT_THUNDER_NUM",
    ),
    (
        "wario",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_WARIO_STATUS_FINAL_WORK_FLOAT_TARGET_MOTION_RATE",
    ),
    (
        "captain",
        crate::data::WorkModuleSetKind::Int,
        "*FIGHTER_CAPTAIN_STATUS_WORK_ID_INT_FALCON_PUNCH_AIR_PHASE",
    ),
    (
        "ganon",
        crate::data::WorkModuleSetKind::Int,
        "*FIGHTER_GANON_STATUS_WORK_ID_INT_GANON_PUNCH_AIR_PHASE",
    ),
    (
        "inkling",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_INKLING_STATUS_SPECIAL_S_WORK_FLOAT_ROLLER_SE_MAX_PITCH",
    ),
    (
        "inkling",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_INKLING_STATUS_SPECIAL_S_WORK_FLOAT_ROLLER_EMPTY_SE_MAX_PITCH",
    ),
    (
        "samus",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_SAMUS_STATUS_FINAL_WORK_FLOAT_OY",
    ),
    (
        "samusd",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_SAMUS_STATUS_FINAL_WORK_FLOAT_OY",
    ),
    (
        "miifighter",
        crate::data::WorkModuleSetKind::Int,
        "*FIGHTER_MIIFIGHTER_STATUS_WORK_ID_INT_BAKURETU_KICK_AIR_PHASE",
    ),
    (
        "demon",
        crate::data::WorkModuleSetKind::Int,
        "*FIGHTER_DEMON_STATUS_ATTACK_S4_WORK_INT_CRITICAL_HIT_NO",
    ),
    (
        "diddy",
        crate::data::WorkModuleSetKind::Int,
        "*FIGHTER_DIDDY_STATUS_FINAL_WORK_INT_WIN_PEANUTS_TYPE",
    ),
    (
        "duckhunt",
        crate::data::WorkModuleSetKind::Int,
        "*FIGHTER_DUCKHUNT_STATUS_ATTACK_INT_SMASH_DELAY_FRAME",
    ),
    (
        "duckhunt",
        crate::data::WorkModuleSetKind::Int,
        "*FIGHTER_DUCKHUNT_STATUS_ATTACK_INT_SMASH_RETICLE_DISPLAY_FRAME",
    ),
    (
        "chrom",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_ROY_STATUS_SPECIAL_HI_WORK_FLOAT_SLIDEGAP_RECOVER_FRAME_INIT",
    ),
    (
        "chrom",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_ROY_STATUS_SPECIAL_HI_WORK_FLOAT_SLIDEGAP_RECOVER_FRAME",
    ),
    (
        "donkey",
        crate::data::WorkModuleSetKind::Int,
        "*FIGHTER_DONKEY_STATUS_SPECIAL_N_WORK_INT_IGNORE_CRITICAL_ATTACK_ID",
    ),
    (
        "gaogaen",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_GAOGAEN_STATUS_SPECIAL_S_FLOAT_LARIAT_HIT_FRAME",
    ),
    (
        "ike",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_IKE_STATUS_SPECIAL_HI_WORK_FLOAT_SLIDEGAP_RECOVER_FRAME_INIT",
    ),
    (
        "ike",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_IKE_STATUS_SPECIAL_HI_WORK_FLOAT_SLIDEGAP_RECOVER_FRAME",
    ),
    (
        "inkling",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_INKLING_STATUS_SPECIAL_S_WORK_FLOAT_WALK_JOSTLE_FRONT",
    ),
    (
        "inkling",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_INKLING_STATUS_SPECIAL_S_WORK_FLOAT_WALK_JOSTLE_BACK",
    ),
    (
        "mario",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_MARIO_STATUS_SPECIAL_S_WORK_ID_FLOAT_REFLECT_MOTION_FRAME",
    ),
    (
        "mariod",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_MARIOD_STATUS_SPECIAL_S_WORK_ID_FLOAT_REFLECT_MOTION_FRAME",
    ),
    (
        "miiswordsman",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_MIISWORDSMAN_STATUS_REVERSE_SLASH_FLOAT_REFLECT_MOTION_FRAME",
    ),
    (
        "pickel",
        crate::data::WorkModuleSetKind::Float,
        "*FIGHTER_PICKEL_STATUS_ATTACK_FLOAT_ATTACK_LW4_MELT_LR",
    ),
];

pub fn is_numeric_source_work_module_set(call: &crate::data::WorkModuleSetCall) -> bool {
    SOURCE_NUMERIC_WORK_SLOTS
        .iter()
        .any(|(_, _, slot)| *slot == call.slot)
}

/// Whether this setter targets a narrowly source-only slot.
/// This is deliberately slot-based so malformed receiver/value variants cannot
/// fall through to a generic registered Work-ID path.
pub fn is_source_only_work_module_set(call: &crate::data::WorkModuleSetCall) -> bool {
    let slot = call.slot.strip_prefix('*').unwrap_or(&call.slot);
    [
        ITEM_GRASS_PULL_NEXT_STEP_SLOT,
        THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX_SLOT,
    ]
    .iter()
    .any(|reviewed| slot == reviewed.strip_prefix('*').unwrap_or(*reviewed))
        || SOURCE_NUMERIC_WORK_SLOTS
            .iter()
            .any(|(_, _, symbol)| symbol.strip_prefix('*') == Some(slot))
        || SOURCE_WORK_MODULE_SET_DESCRIPTORS
            .iter()
            .any(|descriptor| slot == descriptor.slot.strip_prefix('*').unwrap_or(descriptor.slot))
}

/// Validate measured source-only setters or the two ItemGrassPull WorkInt shapes.
/// Other setters continue through the ordinary fighter-owned registry path.
pub fn validate_source_only_work_module_set(
    fighter: &str,
    call: &crate::data::WorkModuleSetCall,
) -> Result<(), String> {
    if !known_source_fighter(fighter) {
        return Err(format!(
            "source-only WorkModule set requires a known concrete fighter, got {fighter:?}"
        ));
    }
    if call.receiver != "agent.module_accessor" {
        return Err("source-only WorkModule setters require agent.module_accessor".into());
    }
    if SOURCE_NUMERIC_WORK_SLOTS
        .iter()
        .any(|(owner, kind, symbol)| {
            *owner == fighter && *kind == call.kind && *symbol == call.slot
        })
    {
        let kind = match call.kind {
            crate::data::WorkModuleSetKind::Int => super::native_calls::ArgumentKind::Integer(&[]),
            crate::data::WorkModuleSetKind::Float => {
                super::native_calls::ArgumentKind::Float(false)
            }
            crate::data::WorkModuleSetKind::Int64 => {
                return Err("Unsupported numeric Work type".into())
            }
        };
        return if kind.accepts(&call.value) {
            Ok(())
        } else {
            Err("Native Work value must be a finite literal of its slot type".into())
        };
    }
    if call.slot == ITEM_GRASS_PULL_NEXT_STEP_SLOT
        || call.slot == THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX_SLOT
    {
        if call.kind != crate::data::WorkModuleSetKind::Int {
            return Err("source-only ItemGrassPull setters require WorkModule::set_int".into());
        }
        let valid_value = if call.slot == ITEM_GRASS_PULL_NEXT_STEP_SLOT {
            ITEM_GRASS_PULL_STEP_VALUES.contains(&call.value.as_str())
        } else {
            call.value == THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX_VALUE
        };
        return if valid_value {
            Ok(())
        } else {
            Err(format!(
                "unsupported source-only ItemGrassPull setter pair: value {:?}, slot {:?}",
                call.value, call.slot
            ))
        };
    }

    let descriptor =
        source_work_module_set_descriptor(fighter, call.kind, &call.slot).ok_or_else(|| {
            format!(
                "unsupported source-only WorkModule setter for {fighter}: {:?} {:?}",
                call.kind, call.slot
            )
        })?;
    if descriptor.values.contains(&call.value.as_str()) {
        Ok(())
    } else {
        Err(format!(
            "unsupported source-only {} value {:?} for slot {:?}",
            descriptor.label, call.value, call.slot
        ))
    }
}

/// Validate the one shared transition-term operation admitted for source-only
/// editing. Transition-term writes do not use the fighter Work-ID registry.
pub fn validate_dash_to_run_transition_term(
    fighter: &str,
    call: &crate::data::WorkTransitionTermCall,
) -> Result<(), String> {
    if registry::fighter_kind_symbol(fighter).is_none() {
        return Err(format!(
            "transition-term editing requires a known concrete fighter, got {fighter:?}"
        ));
    }
    if call.action != crate::data::WorkTransitionTermAction::Enable
        || call.transition_term != DASH_TO_RUN_TERM
        || call.receiver != crate::data::WorkTransitionTermReceiver::AgentAccessor
    {
        return Err(
            "only agent.module_accessor enable_transition_term with *FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN is editable"
                .into(),
        );
    }
    Ok(())
}

/// Validate the measured source-only transition-term family. The shared historical Dash→Run
/// Enable tuple remains available to every concrete fighter; all new tuples are restricted to
/// their exact measured fighter, action, and symbolic token.
pub fn validate_source_transition_term(
    fighter: &str,
    call: &crate::data::WorkTransitionTermCall,
) -> Result<(), String> {
    if registry::fighter_kind_symbol(fighter).is_none() {
        return Err(format!(
            "transition-term editing requires a known concrete fighter, got {fighter:?}"
        ));
    }
    if call.receiver != crate::data::WorkTransitionTermReceiver::AgentAccessor {
        return Err("source-only transition-term editing requires agent.module_accessor".into());
    }
    if call.action == crate::data::WorkTransitionTermAction::Enable
        && call.transition_term == DASH_TO_RUN_TERM
    {
        return validate_dash_to_run_transition_term(fighter, call);
    }
    if source_transition_term_descriptor(fighter, call.action, &call.transition_term).is_some()
        || (registry::fighter_kind_symbol(fighter).is_some()
            && matches!(
                call.action,
                crate::data::WorkTransitionTermAction::Enable
                    | crate::data::WorkTransitionTermAction::Unable
            )
            && COMMON_CANCELS
                .iter()
                .any(|(term, _)| *term == call.transition_term))
    {
        return Ok(());
    }
    Err(format!(
        "unsupported source-only transition term for {fighter}: {:?} {:?}",
        call.action, call.transition_term
    ))
}

/// Reviewed common flag set from `skyline-smash` lua_const revision
/// `258a566618eb5e5540df572d6cbacf48728a1b98` (SHA-256
/// `ac8aa7e32e87a440e8b1f8f99d53270a29a0ee2e9a9bbcd17eb8d71248e1963c`).
///
/// These exact symbolic names are present in pinned `lua_const.rs`; the
/// additional entries have direct cached Game execute examples. Keep the
/// symbols rather than numeric aliases: distinct status constants share Work
/// IDs, and a numeric value alone does not identify the source constant.
const COMMON_FLAGS: &[&str] = &[
    // Legacy reviewed entries: preserve their public/picker ordering.
    "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO",
    "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_RESTART",
    "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_NO_HIT_COMBO",
    "FIGHTER_STATUS_ATTACK_AIR_FLAG_ENABLE_LANDING",
    "FIGHTER_STATUS_CATCH_FLAG_CATCH_WAIT",
    // Additional exact shared status names measured in direct Game execute calls.
    "FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD",
    "FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_CHANGE_STATUS_DLAY_MOTION",
    "FIGHTER_STATUS_ATTACK_FLAG_100_CONTINUE_CHECK",
    "FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_FFLOWER_SHOOT",
    "FIGHTER_STATUS_ITEM_SWING_WORK_FLAG_START_SMASH_HOLD",
    "FIGHTER_STATUS_JUMP_FLY_NEXT",
    // Additional exact shared status names measured in direct Game execute calls.
    "FIGHTER_STATUS_RUN_BRAKE_FLAG_STOP",
    "FIGHTER_STATUS_GENESIS_SHOOT_FLAG_REQUEST_RADIATING",
    "FIGHTER_STATUS_RUN_BRAKE_FLAG_TURN_RUN",
    "FIGHTER_STATUS_DASH_FLAG_TURN_DASH",
    "FIGHTER_STATUS_ESCAPE_AIR_FLAG_SLIDE_ENABLE_CONTROL",
    "FIGHTER_STATUS_ITEM_GRASS_PULL_FLAG_STTUS_END_IF_PICKUP_FAILED",
    "FIGHTER_STATUS_ITEM_SPECIALFLAG_HOIST_WORK_FLAG_HOIST_START",
    "FIGHTER_STATUS_ESCAPE_AIR_FLAG_SLIDE_ENABLE_GRAVITY",
    "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_100",
    // Exact native item, movement, visual-state and instance flags measured in the pinned corpus.
    "FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_SHOOT",
    "FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_AERIAL_REQ_SOUND",
    "FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_AERIAL_REQ_EFFECT",
    "FIGHTER_STATUS_SQUAT_FLAG_REQUEST_SQUAT_SE",
    "FIGHTER_STATUS_VISUAL_SCENE_FLAG_END_EXIT",
    "FIGHTER_STATUS_SUPER_JUMP_PUNCH_FLAG_MOVE_TRANS",
    "FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_GRAVITY_STABLE_UNABLE",
    "FIGHTER_STATUS_ITEM_LIFT_WORK_FLAG_STOP",
    "FIGHTER_STATUS_AIR_LASSO_FLAG_CHECK",
    "FIGHTER_STATUS_SUPER_JUMP_PUNCH_FLAG_REVERSE_LR",
    "FIGHTER_STATUS_VISUAL_SCENE_FLAG_ABS_SET",
    "FIGHTER_STATUS_AIR_LASSO_FLAG_LANDING",
    "FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_LANDING_ATTACK_AIR_SQUAT",
    "FIGHTER_STATUS_STOP_CEIL_FLAG_FALL",
    "FIGHTER_INSTANCE_WORK_ID_FLAG_BACKSHIELD_INVISIBLE",
    "FIGHTER_INSTANCE_WORK_ID_FLAG_NO_SPEED_OPERATION_CHK",
    "FIGHTER_INSTANCE_WORK_ID_FLAG_REVERSE_LR_FINISH_CAMERA_THROW_ORBIT",
    "FIGHTER_INSTANCE_WORK_ID_FLAG_FORCE_LOUPE",
    // Exact recurring common status flags measured in direct cached Game execute calls.
    "FIGHTER_STATUS_ATTACK_AIR_FLAG_LANDING_CLEAR_SPEED",
    "FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_ATTACK_DISABLE_MINI_JUMP_ATTACK",
    "FIGHTER_STATUS_THROW_FLAG_START_AIR",
    "FIGHTER_STATUS_ATTACH_WALL_FLAG_MOT_STOP",
    "FIGHTER_STATUS_TURN_RUN_FLAG_STOP",
    "FIGHTER_STATUS_THROW_FLAG_STOP",
    "FIGHTER_STATUS_ATTACK_AIR_FLAG_KEEP_AIR",
    "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO_INPUT",
    "FIGHTER_STATUS_AIR_LASSO_FLAG_HANG_IMMIDIATE",
    "FIGHTER_STATUS_TURN_RUN_FLAG_TO_RUN",
    // Exact common status checks measured in the pinned cached corpus.
    "FIGHTER_STATUS_GENESIS_SHOOT_FLAG_TURN",
    "FIGHTER_STATUS_JUMP_FLAG_GIMMICK_SPRING_JUMP_FROM_RING",
    "FIGHTER_STATUS_ATTACK_FLAG_SMASH_SMASH_HOLD_TO_ATTACK",
];

/// Exact shared native flags admitted only as source-condition reads, never as writes.
const READ_ONLY_CONDITION_FLAGS: &[&str] = &[
    "FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA",
    "FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_CAPTURE_PULLED_SE",
    "FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FURAFURA_VOICE_FINAL",
    "FIGHTER_INSTANCE_WORK_ID_FLAG_RESULT_FIGHTER_SINGLE",
    "FIGHTER_INSTANCE_WORK_ID_FLAG_RABBIT_CAP",
    "FIGHTER_STATUS_DOWN_FLAG_PREV_STATUS_DOWN_EAT",
];

// Exact condition-only reads observed in the pinned corpus and skyline-smash constants.
// Element's scripts share these symbols; Wolf's Final source reads Fox's caption flag.
// These owner pairs do not extend the writable flag catalogue or live registry.
const OWNED_CONDITION_FLAGS: &[(&str, &[&str])] = &[
    (
        "sonic",
        &[
            "FIGHTER_SONIC_INSTANCE_WORK_FLAG_DISABLE_RUN_TRACE",
            "FIGHTER_SONIC_INSTANCE_WORK_FLAG_SPECIAL_S_HOLD_JUMP_EFFECT",
        ],
    ),
    (
        "fox",
        &["FIGHTER_FOX_REFLECTOR_STATUS_WORK_ID_FLAG_SET_EFFECT"],
    ),
    (
        "eflame",
        &[
            "FIGHTER_ELEMENT_INSTANCE_WORK_ID_FLAG_ENABLE_CHANGE",
            "FIGHTER_ELEMENT_STATUS_SPECIAL_LW_IS_RESULT",
        ],
    ),
    (
        "elight",
        &[
            "FIGHTER_ELEMENT_INSTANCE_WORK_ID_FLAG_ENABLE_CHANGE",
            "FIGHTER_ELEMENT_STATUS_SPECIAL_LW_IS_RESULT",
        ],
    ),
    (
        "wolf",
        &["FIGHTER_FOX_STATUS_WORK_ID_FLAG_FINAL_SPECIAL_CAPTION"],
    ),
];

// Exact weapon-source reads measured in the vanilla corpus and pinned lua_const.rs.
// These pairs authorize conditions only, including explicit shared weapon slots.
const WEAPON_CONDITION_FLAGS: &[(&str, &[&str])] = &[
    (
        "eflame_esword",
        &["WEAPON_EFLAME_ESWORD_STATUS_SPECIAL_S_FLAG_FINISH"],
    ),
    (
        "elight_bunshin",
        &["WEAPON_ELIGHT_BUNSHIN_INSTANCE_WORK_ID_FLAG_FINISH"],
    ),
    (
        "kamui_dragonhand",
        &["WEAPON_KAMUI_DRAGONHAND_INSTANCE_WORK_ID_FLAG_IS_KAMUI"],
    ),
    (
        "ken_hadoken",
        &["WEAPON_RYU_HADOKEN_INSTANCE_WORK_ID_FLAG_COMMAND"],
    ),
    (
        "koopajr_kart",
        &["WEAPON_KOOPAJR_KART_INSTANCE_WORK_ID_FLAG_NO_FLOOR"],
    ),
    (
        "murabito_flowerpot",
        &[
            "FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_AERIAL_REQ_EFFECT",
            "WEAPON_MURABITO_FLOWERPOT_INSTANCE_WORK_ID_FLAG_ASASE",
        ],
    ),
    (
        "murabito_sprinkling_water",
        &["WEAPON_MURABITO_SPRINKLING_WATER_INSTANCE_WORK_ID_FLAG_EFFECT_REQUEST"],
    ),
    (
        "ness_pkflash",
        &["WEAPON_NESS_PK_FLASH_INSTANCE_WORK_ID_FLAG_MAX"],
    ),
    (
        "pacman_bigpacman",
        &["WEAPON_PACMAN_BIGPACMAN_INSTANCE_WORK_ID_FLAG_FLASH"],
    ),
    (
        "pickel_rail",
        &[
            "WEAPON_PICKEL_RAIL_INSTANCE_WORK_ID_FLAG_IS_LEFT",
            "WEAPON_PICKEL_RAIL_INSTANCE_WORK_ID_FLAG_IS_POWERED",
            "WEAPON_PICKEL_RAIL_INSTANCE_WORK_ID_FLAG_WITH_TORCH",
        ],
    ),
    (
        "pickel_trolley",
        &["WEAPON_PICKEL_TROLLEY_INSTANCE_WORK_ID_FLAG_IS_WALL_LEFT"],
    ),
    (
        "rosetta_tico",
        &["WEAPON_ROSETTA_TICO_INSTANCE_WORK_ID_FLAG_DEAD_DAMAGE"],
    ),
    (
        "ryu_hadoken",
        &["WEAPON_RYU_HADOKEN_INSTANCE_WORK_ID_FLAG_COMMAND"],
    ),
    (
        "shizue_pot",
        &["WEAPON_MURABITO_FLOWERPOT_INSTANCE_WORK_ID_FLAG_ASASE"],
    ),
    (
        "tantan_punch1",
        &[
            "WEAPON_TANTAN_PUNCH1_INSTANCE_WORK_ID_FLAG_AIR",
            "WEAPON_TANTAN_PUNCH1_INSTANCE_WORK_ID_FLAG_BOUND",
            "WEAPON_TANTAN_PUNCH1_INSTANCE_WORK_ID_FLAG_CHANGE_HIT_EFFECT",
            "WEAPON_TANTAN_PUNCH1_INSTANCE_WORK_ID_FLAG_IS_DRAGONIZE",
            "WEAPON_TANTAN_PUNCH1_INSTANCE_WORK_ID_FLAG_IS_KIRBY",
        ],
    ),
    (
        "tantan_punch2",
        &[
            "WEAPON_TANTAN_PUNCH1_INSTANCE_WORK_ID_FLAG_AIR",
            "WEAPON_TANTAN_PUNCH1_INSTANCE_WORK_ID_FLAG_BOUND",
        ],
    ),
    (
        "tantan_punch3",
        &[
            "WEAPON_TANTAN_PUNCH1_INSTANCE_WORK_ID_FLAG_AIR",
            "WEAPON_TANTAN_PUNCH1_INSTANCE_WORK_ID_FLAG_BOUND",
        ],
    ),
    (
        "tantan_ring",
        &[
            "WEAPON_TANTAN_RING_INSTANCE_WORK_ID_FLAG_IS_AIR",
            "WEAPON_TANTAN_RING_INSTANCE_WORK_ID_FLAG_IS_LONG",
        ],
    ),
    (
        "trail_hole",
        &["WEAPON_TRAIL_HOLE_INSTANCE_WORK_ID_FLAG_GROUND"],
    ),
    (
        "trail_ice",
        &["WEAPON_TRAIL_ICE_INSTANCE_WORK_ID_FLAG_FROM_POCKET"],
    ),
    (
        "wario_wariobike",
        &["WEAPON_WARIO_WARIOBIKE_INSTANCE_WORK_ID_FLAG_BROKEN_ATTACK"],
    ),
];

/// Exact common fighter-owned flag spellings admitted for source reconstruction.
pub fn common_flags() -> &'static [&'static str] {
    COMMON_FLAGS
}

/// Closed choices for native read-only condition pickers. These are deliberately separate from
/// `common_flags()` so they cannot enter generic writable WorkFlag controls.
pub fn read_only_condition_flags() -> &'static [&'static str] {
    READ_ONLY_CONDITION_FLAGS
}

/// Source-condition choices are independent of writable/runtime registry entries. Nana's
/// common sound and Final sources use these shared reads despite having no retained fighter
/// kind mapping in the runtime registry. This does not alias her runtime owner to Popo.
pub fn read_only_condition_flag_choices(fighter: &str) -> Vec<&'static str> {
    if let Some((_, flags)) = WEAPON_CONDITION_FLAGS
        .iter()
        .find(|(owner, _)| *owner == fighter)
    {
        return flags.to_vec();
    }
    if !known_source_fighter(fighter) {
        return Vec::new();
    }
    READ_ONLY_CONDITION_FLAGS
        .iter()
        .copied()
        .chain(
            OWNED_CONDITION_FLAGS
                .iter()
                .filter(|(owner, _)| *owner == fighter)
                .flat_map(|(_, flags)| flags.iter().copied()),
        )
        .collect()
}

// Native scripts for derivative fighters share these exact flag catalogues. This
// source-only lookup retains the symbolic ID; it does not alias live registry owners.
const SHARED_FLAG_OWNERS: &[(&str, &[&str])] = &[
    ("ken", &["ryu"]),
    ("lucina", &["marth"]),
    ("roy", &["marth"]),
    ("chrom", &["roy", "marth"]),
    ("samusd", &["samus"]),
    ("younglink", &["link"]),
    ("toonlink", &["link"]),
    ("koopag", &["koopa"]),
    ("richter", &["simon"]),
    ("pitb", &["pit"]),
    ("nana", &["popo"]),
];

fn shared_flag_owners(fighter: &str) -> &'static [&'static str] {
    SHARED_FLAG_OWNERS
        .iter()
        .find(|(owner, _)| *owner == fighter)
        .map_or(&[], |(_, owners)| *owners)
}

fn known_source_fighter(fighter: &str) -> bool {
    registry::fighter_kind_symbol(fighter).is_some() || fighter == "nana"
}

pub fn is_borrowed_registry_flag(fighter: &str, symbol: &str) -> bool {
    registry::resolve(fighter, symbol, IdentifierKind::WorkFlag).is_err()
        && shared_flag_owners(fighter)
            .iter()
            .any(|owner| registry::resolve(owner, symbol, IdentifierKind::WorkFlag).is_ok())
}

/// Whether a flag is admitted only for native-source reconstruction and must
/// not be represented by the flat live ACMD protocol.
pub fn is_source_only_flag_symbol(symbol: &str) -> bool {
    COMMON_FLAGS.contains(&symbol)
        || READ_ONLY_CONDITION_FLAGS.contains(&symbol)
        || OWNED_CONDITION_FLAGS
            .iter()
            .any(|(_, flags)| flags.contains(&symbol))
        || WEAPON_CONDITION_FLAGS
            .iter()
            .any(|(_, flags)| flags.contains(&symbol))
        || super::native_status_flags::contains_symbol(symbol)
}

/// Validate an exact native flag used only as a read-only `MoveCondition` leaf. Existing
/// registered/common/status conditions retain the normal validator behavior; this read-only
/// catalog is not admitted by `validate_flag`, which remains the write-command validator.
pub fn validate_condition_flag(fighter: &str, token: &str) -> Result<String, String> {
    // Preserve every existing owner-valid read before considering condition-only fallbacks.
    if let Ok(symbol) = validate_flag(fighter, token) {
        return Ok(symbol);
    }
    let symbol = dereferenced_symbol(token)?;
    if READ_ONLY_CONDITION_FLAGS.contains(&symbol)
        || OWNED_CONDITION_FLAGS
            .iter()
            .any(|(_, flags)| flags.contains(&symbol))
        || WEAPON_CONDITION_FLAGS
            .iter()
            .any(|(_, flags)| flags.contains(&symbol))
    {
        if !read_only_condition_flag_choices(fighter).contains(&symbol) {
            return Err(format!(
                "read-only condition flag {symbol:?} requires a reviewed source owner; {fighter:?} has no exact mapping"
            ));
        }
        return Ok(symbol.to_owned());
    }
    validate_flag(fighter, token)
}

/// Validate one exact dereferenced WorkFlag token and return its bare symbol.
///
/// Registered instance flags keep their existing exact lookup behavior. The
/// source-only fallbacks require either an exact common spelling or an exact
/// `(fighter, symbol)` catalogue entry, plus a concrete fighter-kind mapping.
pub fn validate_flag(fighter: &str, token: &str) -> Result<String, String> {
    let symbol = dereferenced_symbol(token)?;
    if known_source_fighter(fighter)
        && shared_flag_owners(fighter).iter().any(|owner| {
            registry::resolve(owner, symbol, IdentifierKind::WorkFlag).is_ok()
                || super::native_status_flags::resolve(owner, symbol).is_some()
        })
    {
        return Ok(symbol.to_owned());
    }
    match registry::resolve(fighter, symbol, IdentifierKind::WorkFlag) {
        Ok(entry) => return Ok(entry.symbol.to_owned()),
        Err(registry_error) => {
            if let Some(owner) = super::native_status_flags::owner_of(symbol) {
                if super::native_status_flags::resolve(fighter, symbol).is_none() {
                    return Err(format!(
                        "status work flag {symbol:?} belongs to {owner:?}, not {fighter:?}"
                    ));
                }
                if !known_source_fighter(fighter) {
                    return Err(format!(
                        "status work flag {symbol:?} requires an exact fighter kind for {fighter:?}"
                    ));
                }
                return Ok(symbol.to_owned());
            }
            if !COMMON_FLAGS.contains(&symbol) {
                return Err(format!(
                    "work flag {symbol:?} rejected for {fighter:?}: {registry_error}"
                ));
            }
            if !known_source_fighter(fighter) {
                return Err(format!(
                    "common work flag {symbol:?} requires a known concrete fighter kind; {fighter:?} has no exact mapping"
                ));
            }
        }
    }
    Ok(symbol.to_owned())
}

fn dereferenced_symbol(token: &str) -> Result<&str, String> {
    let symbol = token.strip_prefix('*').ok_or_else(|| {
        "work identifier must use a dereferenced lua constant (*SYMBOL)".to_owned()
    })?;
    if symbol.is_empty()
        || !symbol
            .bytes()
            .enumerate()
            .all(|(index, byte)| byte.is_ascii_alphanumeric() || byte == b'_' && index > 0)
        || !symbol.as_bytes()[0].is_ascii_alphabetic() && symbol.as_bytes()[0] != b'_'
    {
        return Err("work identifier must be one exact named lua constant".into());
    }
    Ok(symbol)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_native_work_slots_validate_owner_type_and_source_roundtrip() {
        use crate::move_nodes::{
            model::{MoveGraph, NodeKind, Track},
            timeline,
        };
        for (fighter, category, statement) in [
            ("littlemac", "game", "WorkModule::set_float(agent.module_accessor, 10.0, *FIGHTER_LITTLEMAC_STATUS_SPECIAL_N_FLOAT_POWER_MIN);"),
            ("gamewatch", "expression", "WorkModule::set_float(agent.module_accessor, -50.0, *FIGHTER_GAMEWATCH_STATUS_THROW_WORK_FLOAT_STOCK_ICON_ROTATE);"),
            ("yoshi", "game", "WorkModule::set_int(agent.module_accessor, 2, *FIGHTER_YOSHI_STATUS_FINAL_WORK_INT_CHANGE_MOTION_TARGET);"),
        ] {
            let source = format!("unsafe extern \"C\" fn {category}_test(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {statement}\n    }}\n}}");
            let script = if category == "game" { crate::acmd::parse_acmd_script(&source) } else { crate::acmd::parse_expression_script(&source) };
            let track = if category == "game" { Track::Game } else { Track::Expression };
            let imported = timeline::import_for_fighter(fighter, track, &script);
            assert!(matches!(imported.nodes[0].kind, NodeKind::Execute { .. }));
            let graph = MoveGraph { tracks: vec![imported], ..Default::default() };
            let restored = serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
            let compiled = timeline::compile(fighter, "test", &restored).unwrap();
            assert_eq!(serde_json::to_value(&script).unwrap(), serde_json::to_value(&compiled[0].1).unwrap());
            assert!(crate::move_nodes::live_support::source_only_tracks(&graph).next().is_some());
            let crate::data::AcmdStmt::Excute(commands) = &script.stmts[0] else { panic!() };
            let crate::data::ExcuteStmt::WorkModuleSet(call) = &commands[0] else { panic!() };
            assert!(validate_source_only_work_module_set("mario", call).is_err());
            let mut invalid = call.clone();
            invalid.value = "injected()".into();
            assert!(validate_source_only_work_module_set(fighter, &invalid).is_err());
            invalid.value = "NaN".into();
            assert!(validate_source_only_work_module_set(fighter, &invalid).is_err());
        }
    }

    #[test]
    fn shared_native_flags_keep_source_owner_and_track_without_live_aliases() {
        use crate::move_nodes::{
            model::{MoveGraph, NodeKind, Track},
            timeline,
        };
        for (fighter, track, symbol) in [
            (
                "ken",
                Track::Game,
                "FIGHTER_RYU_INSTANCE_WORK_ID_FLAG_FINAL_HIT_CANCEL",
            ),
            (
                "lucina",
                Track::Game,
                "FIGHTER_MARTH_STATUS_SPECIAL_S_FLAG_INPUT_CHECK",
            ),
            (
                "nana",
                Track::Game,
                "FIGHTER_STATUS_ATTACK_AIR_FLAG_ENABLE_LANDING",
            ),
            (
                "mario",
                Track::Sound,
                "FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_AERIAL_REQ_SOUND",
            ),
            (
                "mario",
                Track::Expression,
                "FIGHTER_INSTANCE_WORK_ID_FLAG_BACKSHIELD_INVISIBLE",
            ),
        ] {
            let category = match track {
                Track::Sound => "sound",
                Track::Expression => "expression",
                _ => "game",
            };
            let source = format!("unsafe extern \"C\" fn {category}_test(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        WorkModule::off_flag(agent.module_accessor, *{symbol});\n    }}\n}}");
            let parse = match track {
                Track::Sound => crate::acmd::parse_sound_script,
                Track::Expression => crate::acmd::parse_expression_script,
                _ => crate::acmd::parse_acmd_script,
            };
            let script = parse(&source);
            let imported = timeline::import_for_fighter(fighter, track, &script);
            assert!(
                matches!(imported.nodes[0].kind, NodeKind::Execute { .. }),
                "{source}"
            );
            let graph = MoveGraph {
                tracks: vec![imported],
                ..Default::default()
            };
            let restored = serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
            let compiled = timeline::compile(fighter, "test", &restored).unwrap();
            let emitted = match track {
                Track::Sound => crate::acmd::preview_sound_fn(&compiled[0].1, "test"),
                Track::Expression => crate::acmd::preview_expression_fn(&compiled[0].1, "test"),
                _ => crate::acmd::preview_game_fn(&compiled[0].1, "test"),
            };
            assert_eq!(
                serde_json::to_value(parse(&emitted)).unwrap(),
                serde_json::to_value(&script).unwrap()
            );
            assert!(crate::move_nodes::live_support::source_only_tracks(&graph)
                .next()
                .is_some());
        }
        assert!(validate_flag(
            "mario",
            "*FIGHTER_RYU_INSTANCE_WORK_ID_FLAG_FINAL_HIT_CANCEL"
        )
        .is_err());
        assert!(validate_flag("ken", "*FIGHTER_RYU_INSTANCE_WORK_ID_FLAG_UNKNOWN").is_err());
        assert!(!is_source_only_flag_symbol(
            "FIGHTER_RYU_INSTANCE_WORK_ID_FLAG_FINAL_HIT_CANCEL"
        ));
        assert!(registry::resolve(
            "ken",
            "FIGHTER_RYU_INSTANCE_WORK_ID_FLAG_FINAL_HIT_CANCEL",
            IdentifierKind::WorkFlag
        )
        .is_err());
    }

    #[test]
    fn dash_to_run_transition_term_is_exact_and_source_only() {
        use crate::data::{
            WorkTransitionTermAction as Action, WorkTransitionTermCall,
            WorkTransitionTermReceiver as Receiver,
        };

        let call = |action, transition_term: &str| WorkTransitionTermCall {
            action,
            receiver: Receiver::AgentAccessor,
            transition_term: transition_term.into(),
        };
        let accepted = call(
            Action::Enable,
            "*FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN",
        );
        for fighter in ["cloud", "mario", "fox", "link"] {
            assert!(
                validate_dash_to_run_transition_term(fighter, &accepted).is_ok(),
                "measured shared transition term rejected for {fighter}"
            );
        }
        for (fighter, invalid) in [
            (
                "mario",
                call(
                    Action::Unable,
                    "*FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN",
                ),
            ),
            ("mario", call(Action::Enable, "17")),
            (
                "mario",
                call(
                    Action::Enable,
                    "FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN",
                ),
            ),
            (
                "mario",
                call(
                    Action::Enable,
                    "*FIGHTER_MARIO_STATUS_SPECIAL_S_TRANSITION_TERM_ID_GROUND",
                ),
            ),
            ("not_a_fighter", accepted.clone()),
        ] {
            assert!(
                validate_dash_to_run_transition_term(fighter, &invalid).is_err(),
                "accepted invalid transition term for {fighter}: {:?}",
                invalid.transition_term
            );
        }

        let boma = WorkTransitionTermCall {
            receiver: Receiver::Boma,
            ..accepted.clone()
        };
        assert!(validate_dash_to_run_transition_term("mario", &boma).is_err());

        let old_saved = r#"{"action":"Enable","transition_term":"*FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN"}"#;
        let decoded: WorkTransitionTermCall = serde_json::from_str(old_saved).unwrap();
        assert_eq!(decoded, accepted);
        assert_eq!(serde_json::to_string(&decoded).unwrap(), old_saved);

        let boma = WorkTransitionTermCall {
            receiver: Receiver::Boma,
            ..accepted
        };
        let saved_boma = serde_json::to_string(&boma).unwrap();
        assert!(saved_boma.contains(r#""receiver":"Boma""#));
        assert_eq!(
            serde_json::from_str::<WorkTransitionTermCall>(&saved_boma).unwrap(),
            boma
        );
        assert_eq!(boma.receiver.token(), "boma");
    }

    #[test]
    fn measured_transition_term_descriptors_are_exact_concrete_tuples() {
        use crate::data::{
            WorkTransitionTermAction as Action, WorkTransitionTermCall,
            WorkTransitionTermReceiver as Receiver,
        };

        assert_eq!(source_transition_term_descriptors().len(), 20);
        for descriptor in source_transition_term_descriptors() {
            let choices = source_transition_term_choices(descriptor.fighter);
            assert!(choices.iter().any(|choice| {
                choice.fighter == descriptor.fighter
                    && choice.action == Action::Enable
                    && choice.transition_term == DASH_TO_RUN_TERM
            }));
            assert!(choices.iter().any(|choice| {
                choice.fighter == descriptor.fighter
                    && choice.action == descriptor.action
                    && choice.transition_term == descriptor.transition_term
            }));
            let call = WorkTransitionTermCall {
                action: descriptor.action,
                receiver: Receiver::AgentAccessor,
                transition_term: descriptor.transition_term.into(),
            };
            assert!(
                registry::fighter_kind_symbol(descriptor.fighter).is_some(),
                "descriptor owner is not a concrete registered fighter: {}",
                descriptor.fighter
            );
            assert!(
                validate_source_transition_term(descriptor.fighter, &call).is_ok(),
                "measured tuple rejected: {} {:?} {}",
                descriptor.fighter,
                descriptor.action,
                descriptor.transition_term
            );
            assert!(source_transition_term_descriptor(
                descriptor.fighter,
                descriptor.action,
                descriptor.transition_term
            )
            .is_some());

            let wrong_action = match descriptor.action {
                Action::Enable => Action::Unable,
                Action::Unable => Action::EnableGroup,
                Action::EnableGroup => Action::Enable,
                Action::UnableGroupEx => Action::EnableGroup,
            };
            let mismatched_action = WorkTransitionTermCall {
                action: wrong_action,
                ..call.clone()
            };
            // Everyday cancels may be allowed or blocked on any move.
            let common = COMMON_CANCELS
                .iter()
                .any(|(term, _)| *term == descriptor.transition_term);
            assert!(
                common
                    || validate_source_transition_term(descriptor.fighter, &mismatched_action)
                        .is_err()
            );

            let wrong_owner = if descriptor.fighter == "mario" {
                "fox"
            } else {
                "mario"
            };
            assert!(common || validate_source_transition_term(wrong_owner, &call).is_err());

            let wrong_symbol = WorkTransitionTermCall {
                transition_term: "*UNMEASURED_TRANSITION_TERM".into(),
                ..call.clone()
            };
            assert!(validate_source_transition_term(descriptor.fighter, &wrong_symbol).is_err());

            let wrong_receiver = WorkTransitionTermCall {
                receiver: Receiver::Boma,
                ..call
            };
            assert!(validate_source_transition_term(descriptor.fighter, &wrong_receiver).is_err());

            assert!(!descriptor.label.trim().is_empty());
        }

        // Owner-specific terms can have the same pinned numeric ID; the source token and
        // concrete owner, not that numeric alias, determine admission.
        let luigi_term = "*FIGHTER_LUIGI_STATUS_SPECIAL_S_RAM_TRANSITION_TERM_ID_GROUND";
        let luigi_call = WorkTransitionTermCall {
            action: Action::Enable,
            receiver: Receiver::AgentAccessor,
            transition_term: luigi_term.into(),
        };
        assert!(validate_source_transition_term("luigi", &luigi_call).is_ok());
        assert!(validate_source_transition_term("diddy", &luigi_call).is_err());
        assert!(source_transition_term_choices("not_a_fighter").is_empty());
        // Every fighter can allow or block the everyday cancels.
        for action in [Action::Enable, Action::Unable] {
            let call = WorkTransitionTermCall {
                action,
                receiver: Receiver::AgentAccessor,
                transition_term: COMMON_CANCELS[0].0.into(),
            };
            assert!(validate_source_transition_term("donkey", &call).is_ok());
            assert!(validate_source_transition_term("not_a_fighter", &call).is_err());
        }
    }

    #[test]
    fn item_grass_pull_setters_admit_only_the_measured_int_pairs() {
        use crate::data::{WorkModuleSetCall, WorkModuleSetKind};

        let call = |value: &str, slot: &str| WorkModuleSetCall {
            kind: WorkModuleSetKind::Int,
            receiver: "agent.module_accessor".into(),
            value: value.into(),
            slot: slot.into(),
        };
        for value in ITEM_GRASS_PULL_STEP_VALUES {
            let step = call(value, ITEM_GRASS_PULL_NEXT_STEP_SLOT);
            assert!(is_source_only_work_module_set(&step));
            for fighter in ["cloud", "mario", "fox", "link"] {
                assert!(validate_source_only_work_module_set(fighter, &step).is_ok());
            }
        }
        let offset = call(
            THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX_VALUE,
            THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX_SLOT,
        );
        assert!(is_source_only_work_module_set(&offset));
        assert!(validate_source_only_work_module_set("cloud", &offset).is_ok());

        for invalid in [
            call(
                "*FIGHTER_ITEM_GRASS_PULL_STEP_OTHER",
                ITEM_GRASS_PULL_NEXT_STEP_SLOT,
            ),
            call("4", ITEM_GRASS_PULL_NEXT_STEP_SLOT),
            call("5", THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX_SLOT),
            call("4", ITEM_GRASS_PULL_NEXT_STEP_SLOT),
            call(
                "*FIGHTER_ITEM_GRASS_PULL_STEP_PICKUP",
                THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX_SLOT,
            ),
        ] {
            assert!(is_source_only_work_module_set(&invalid));
            assert!(validate_source_only_work_module_set("cloud", &invalid).is_err());
        }
        for invalid in [
            WorkModuleSetCall {
                receiver: "boma".into(),
                ..call(
                    ITEM_GRASS_PULL_STEP_VALUES[0],
                    ITEM_GRASS_PULL_NEXT_STEP_SLOT,
                )
            },
            WorkModuleSetCall {
                kind: WorkModuleSetKind::Float,
                ..offset.clone()
            },
            WorkModuleSetCall {
                slot: ITEM_GRASS_PULL_NEXT_STEP_SLOT
                    .trim_start_matches('*')
                    .into(),
                ..call(
                    ITEM_GRASS_PULL_STEP_VALUES[0],
                    ITEM_GRASS_PULL_NEXT_STEP_SLOT,
                )
            },
        ] {
            assert!(is_source_only_work_module_set(&invalid));
            assert!(validate_source_only_work_module_set("cloud", &invalid).is_err());
        }
        assert!(validate_source_only_work_module_set("not_a_fighter", &offset).is_err());
        assert!(!is_source_only_work_module_set(&call(
            "4",
            "*FIGHTER_INSTANCE_WORK_ID_INT_X"
        )));
    }

    #[test]
    fn measured_wave_angle_and_open_motion_pairs_are_owner_and_type_closed() {
        use crate::data::{WorkModuleSetCall, WorkModuleSetKind};

        let call = |kind, fighter_receiver: &str, value: &str, slot: &str| WorkModuleSetCall {
            kind,
            receiver: fighter_receiver.into(),
            value: value.into(),
            slot: slot.into(),
        };
        let wave_slot = "*FIGHTER_MIISWORDSMAN_STATUS_FINAL_WORK_ID_FLOAT_WAVE_ANGLE";
        let motion_slot = "*FIGHTER_ELIGHT_INSTANCE_WORK_ID_INT_ESWORD_INHERIT_OPEN_MOTION_KIND";

        let wave =
            source_work_module_set_descriptor("miiswordsman", WorkModuleSetKind::Float, wave_slot)
                .expect("measured Mii Swordfighter slot descriptor");
        assert_eq!(wave.values, MIISWORDSMAN_WAVE_ANGLE_VALUES);
        assert_eq!(wave.values.len(), 13);
        for value in wave.values {
            assert!(validate_source_only_work_module_set(
                "miiswordsman",
                &call(
                    WorkModuleSetKind::Float,
                    "agent.module_accessor",
                    value,
                    wave_slot
                ),
            )
            .is_ok());
        }
        assert!(
            source_work_module_set_descriptor("mario", WorkModuleSetKind::Float, wave_slot)
                .is_none()
        );
        assert!(source_work_module_set_descriptor(
            "miiswordsman",
            WorkModuleSetKind::Int,
            wave_slot
        )
        .is_none());

        let eflame =
            source_work_module_set_descriptor("eflame", WorkModuleSetKind::Int64, motion_slot)
                .expect("measured Eflame slot descriptor");
        let elight =
            source_work_module_set_descriptor("elight", WorkModuleSetKind::Int64, motion_slot)
                .expect("measured Elight slot descriptor");
        assert_eq!(eflame.values.len(), 12);
        assert_eq!(elight.values.len(), 11);
        for value in eflame.values {
            assert!(validate_source_only_work_module_set(
                "eflame",
                &call(
                    WorkModuleSetKind::Int64,
                    "agent.module_accessor",
                    value,
                    motion_slot
                ),
            )
            .is_ok());
        }
        for value in elight.values {
            assert!(validate_source_only_work_module_set(
                "elight",
                &call(
                    WorkModuleSetKind::Int64,
                    "agent.module_accessor",
                    value,
                    motion_slot
                ),
            )
            .is_ok());
        }
        assert!(!eflame
            .values
            .contains(&"hash40(\"attack_hi4_hold\") as i64"));
        assert!(!elight.values.contains(&"hash40(\"special_air_n4\") as i64"));

        let invalid = [
            (
                "eflame",
                call(
                    WorkModuleSetKind::Int64,
                    "agent.module_accessor",
                    "hash40(\"attack_hi4_hold\") as i64",
                    motion_slot,
                ),
            ),
            (
                "elight",
                call(
                    WorkModuleSetKind::Int64,
                    "agent.module_accessor",
                    "hash40(\"special_air_n4\") as i64",
                    motion_slot,
                ),
            ),
            (
                "eflame",
                call(
                    WorkModuleSetKind::Float,
                    "agent.module_accessor",
                    "1.0",
                    motion_slot,
                ),
            ),
            (
                "miiswordsman",
                call(
                    WorkModuleSetKind::Int,
                    "agent.module_accessor",
                    "0",
                    wave_slot,
                ),
            ),
            (
                "miiswordsman",
                call(WorkModuleSetKind::Float, "boma", "0.0", wave_slot),
            ),
            (
                "elight",
                call(
                    WorkModuleSetKind::Int64,
                    "boma",
                    "hash40(\"final_air_attack\") as i64",
                    motion_slot,
                ),
            ),
            (
                "not_a_fighter",
                call(
                    WorkModuleSetKind::Int64,
                    "agent.module_accessor",
                    "hash40(\"final_air_attack\") as i64",
                    motion_slot,
                ),
            ),
            (
                "miiswordsman",
                call(
                    WorkModuleSetKind::Float,
                    "agent.module_accessor",
                    "0.0 + 1.0",
                    wave_slot,
                ),
            ),
        ];
        for (fighter, invalid) in invalid {
            assert!(is_source_only_work_module_set(&invalid));
            assert!(validate_source_only_work_module_set(fighter, &invalid).is_err());
        }
        assert!(is_source_only_work_module_set(&call(
            WorkModuleSetKind::Float,
            "agent.module_accessor",
            "0.0",
            wave_slot.trim_start_matches('*'),
        )));
        assert!(validate_source_only_work_module_set(
            "miiswordsman",
            &call(
                WorkModuleSetKind::Float,
                "agent.module_accessor",
                "0.0",
                wave_slot.trim_start_matches('*'),
            ),
        )
        .is_err());
    }

    const CACHE_FLAGS: [&str; 6] = [
        "FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD",
        "FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_CHANGE_STATUS_DLAY_MOTION",
        "FIGHTER_STATUS_ATTACK_FLAG_100_CONTINUE_CHECK",
        "FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_FFLOWER_SHOOT",
        "FIGHTER_STATUS_ITEM_SWING_WORK_FLAG_START_SMASH_HOLD",
        "FIGHTER_STATUS_JUMP_FLY_NEXT",
    ];

    fn cache_flag_index(call: &crate::data::WorkFlagCall) -> Option<usize> {
        let symbol = call.flag.strip_prefix('*').unwrap_or(&call.flag);
        CACHE_FLAGS
            .iter()
            .position(|candidate| *candidate == symbol)
    }

    fn count_script_flags(
        stmts: &[crate::data::AcmdStmt],
        direct: &mut [usize; 6],
        opaque_or_loop: &mut [usize; 6],
        bare: &mut [usize; 6],
        nested: bool,
    ) {
        use crate::data::{AcmdStmt, ExcuteStmt};

        for stmt in stmts {
            match stmt {
                AcmdStmt::Excute(commands) => {
                    for command in commands {
                        if let ExcuteStmt::WorkFlag(call) = command {
                            if let Some(index) = cache_flag_index(call) {
                                if nested {
                                    opaque_or_loop[index] += 1;
                                } else {
                                    direct[index] += 1;
                                }
                            }
                        }
                    }
                }
                AcmdStmt::Bare(command) => {
                    if let ExcuteStmt::WorkFlag(call) = command.as_ref() {
                        if let Some(index) = cache_flag_index(call) {
                            bare[index] += 1;
                        }
                    }
                }
                AcmdStmt::RawBlock { body, .. } | AcmdStmt::Loop { body, .. } => {
                    count_script_flags(body, direct, opaque_or_loop, bare, true);
                }
                _ => {}
            }
        }
    }

    fn count_editable_flags(
        nodes: &[crate::move_nodes::model::MoveNode],
        editable: &mut [usize; 6],
        preserved: &mut [usize; 6],
    ) {
        use crate::data::ExcuteStmt;
        use crate::move_nodes::model::{MoveCommand, NodeKind};

        for node in nodes {
            match &node.kind {
                NodeKind::Execute { commands } => {
                    for command in commands {
                        if let ExcuteStmt::WorkFlag(call) = command {
                            if let Some(index) = cache_flag_index(call) {
                                editable[index] += 1;
                            }
                        }
                    }
                }
                NodeKind::MixedExecute { commands } => {
                    for command in commands {
                        match command {
                            MoveCommand::Editable(ExcuteStmt::WorkFlag(call)) => {
                                if let Some(index) = cache_flag_index(call) {
                                    editable[index] += 1;
                                }
                            }
                            MoveCommand::Preserved(ExcuteStmt::WorkFlag(call)) => {
                                if let Some(index) = cache_flag_index(call) {
                                    preserved[index] += 1;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                NodeKind::Repeat { body, .. } => count_editable_flags(body, editable, preserved),
                NodeKind::Branch {
                    then_nodes,
                    else_nodes,
                    ..
                } => {
                    count_editable_flags(then_nodes, editable, preserved);
                    count_editable_flags(else_nodes, editable, preserved);
                }
                NodeKind::Preserved { .. }
                | NodeKind::Frame { .. }
                | NodeKind::NativeExecuteFrame { .. }
                | NodeKind::NativeMotionLoopSync
                | NodeKind::NativeWaitLoopClear
                | NodeKind::Wait { .. }
                | NodeKind::MotionRate { .. }
                | NodeKind::BareCommand { .. }
                | NodeKind::EffectExecute { .. }
                | NodeKind::MixedEffectExecute { .. } => {}
            }
        }
    }

    fn has_scoped_scope_condition(nodes: &[crate::move_nodes::model::MoveNode]) -> bool {
        use crate::data::AcmdStmt;
        use crate::move_nodes::model::NodeKind;

        nodes.iter().any(|node| match &node.kind {
            NodeKind::Preserved {
                statement: AcmdStmt::RawBlock { header, body },
            } => {
                header.contains("FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_SHOOT")
                    || body.iter().any(|stmt| matches!(
                        stmt,
                        AcmdStmt::Raw(line) if line.contains("FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_SHOOT")
                    ))
            }
            NodeKind::Repeat { body, .. } => has_scoped_scope_condition(body),
            NodeKind::Branch {
                condition: crate::move_nodes::model::MoveCondition::WorkFlag { symbol, .. },
                ..
            } if symbol == "FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_SHOOT" => true,
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => {
                has_scoped_scope_condition(then_nodes)
                    || has_scoped_scope_condition(else_nodes)
            }
            _ => false,
        })
    }

    #[test]
    fn reviewed_common_flags_accept_only_known_concrete_fighters() {
        for symbol in common_flags() {
            for fighter in ["mario", "cloud"] {
                assert_eq!(
                    validate_flag(fighter, &format!("*{symbol}")).unwrap(),
                    *symbol
                );
            }
            for fighter in ["pokemon", "shield"] {
                assert!(
                    validate_flag(fighter, &format!("*{symbol}")).is_err(),
                    "common flag {symbol} was accepted for unmapped fighter {fighter}"
                );
            }
        }
    }

    #[test]
    fn common_flags_remain_a_small_explicit_set() {
        assert_eq!(
            common_flags(),
            &[
                "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO",
                "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_RESTART",
                "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_NO_HIT_COMBO",
                "FIGHTER_STATUS_ATTACK_AIR_FLAG_ENABLE_LANDING",
                "FIGHTER_STATUS_CATCH_FLAG_CATCH_WAIT",
                "FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD",
                "FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_CHANGE_STATUS_DLAY_MOTION",
                "FIGHTER_STATUS_ATTACK_FLAG_100_CONTINUE_CHECK",
                "FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_FFLOWER_SHOOT",
                "FIGHTER_STATUS_ITEM_SWING_WORK_FLAG_START_SMASH_HOLD",
                "FIGHTER_STATUS_JUMP_FLY_NEXT",
                "FIGHTER_STATUS_RUN_BRAKE_FLAG_STOP",
                "FIGHTER_STATUS_GENESIS_SHOOT_FLAG_REQUEST_RADIATING",
                "FIGHTER_STATUS_RUN_BRAKE_FLAG_TURN_RUN",
                "FIGHTER_STATUS_DASH_FLAG_TURN_DASH",
                "FIGHTER_STATUS_ESCAPE_AIR_FLAG_SLIDE_ENABLE_CONTROL",
                "FIGHTER_STATUS_ITEM_GRASS_PULL_FLAG_STTUS_END_IF_PICKUP_FAILED",
                "FIGHTER_STATUS_ITEM_SPECIALFLAG_HOIST_WORK_FLAG_HOIST_START",
                "FIGHTER_STATUS_ESCAPE_AIR_FLAG_SLIDE_ENABLE_GRAVITY",
                "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_100",
                "FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_SHOOT",
                "FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_AERIAL_REQ_SOUND",
                "FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_AERIAL_REQ_EFFECT",
                "FIGHTER_STATUS_SQUAT_FLAG_REQUEST_SQUAT_SE",
                "FIGHTER_STATUS_VISUAL_SCENE_FLAG_END_EXIT",
                "FIGHTER_STATUS_SUPER_JUMP_PUNCH_FLAG_MOVE_TRANS",
                "FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_GRAVITY_STABLE_UNABLE",
                "FIGHTER_STATUS_ITEM_LIFT_WORK_FLAG_STOP",
                "FIGHTER_STATUS_AIR_LASSO_FLAG_CHECK",
                "FIGHTER_STATUS_SUPER_JUMP_PUNCH_FLAG_REVERSE_LR",
                "FIGHTER_STATUS_VISUAL_SCENE_FLAG_ABS_SET",
                "FIGHTER_STATUS_AIR_LASSO_FLAG_LANDING",
                "FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_LANDING_ATTACK_AIR_SQUAT",
                "FIGHTER_STATUS_STOP_CEIL_FLAG_FALL",
                "FIGHTER_INSTANCE_WORK_ID_FLAG_BACKSHIELD_INVISIBLE",
                "FIGHTER_INSTANCE_WORK_ID_FLAG_NO_SPEED_OPERATION_CHK",
                "FIGHTER_INSTANCE_WORK_ID_FLAG_REVERSE_LR_FINISH_CAMERA_THROW_ORBIT",
                "FIGHTER_INSTANCE_WORK_ID_FLAG_FORCE_LOUPE",
                "FIGHTER_STATUS_ATTACK_AIR_FLAG_LANDING_CLEAR_SPEED",
                "FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_ATTACK_DISABLE_MINI_JUMP_ATTACK",
                "FIGHTER_STATUS_THROW_FLAG_START_AIR",
                "FIGHTER_STATUS_ATTACH_WALL_FLAG_MOT_STOP",
                "FIGHTER_STATUS_TURN_RUN_FLAG_STOP",
                "FIGHTER_STATUS_THROW_FLAG_STOP",
                "FIGHTER_STATUS_ATTACK_AIR_FLAG_KEEP_AIR",
                "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO_INPUT",
                "FIGHTER_STATUS_AIR_LASSO_FLAG_HANG_IMMIDIATE",
                "FIGHTER_STATUS_TURN_RUN_FLAG_TO_RUN",
                "FIGHTER_STATUS_GENESIS_SHOOT_FLAG_TURN",
                "FIGHTER_STATUS_JUMP_FLAG_GIMMICK_SPRING_JUMP_FROM_RING",
                "FIGHTER_STATUS_ATTACK_FLAG_SMASH_SMASH_HOLD_TO_ATTACK",
            ]
        );
    }

    #[test]
    fn common_condition_reads_never_authorize_writes_or_unknown_symbols() {
        for symbol in read_only_condition_flags() {
            for fighter in ["mario", "bayonetta", "nana"] {
                assert_eq!(
                    validate_condition_flag(fighter, &format!("*{symbol}")).as_deref(),
                    Ok(*symbol)
                );
                assert!(validate_flag(fighter, &format!("*{symbol}")).is_err());
            }
            for token in [
                symbol.to_string(),
                format!("*{symbol}_UNKNOWN"),
                format!("*{symbol} + 1"),
            ] {
                assert!(validate_condition_flag("mario", &token).is_err());
            }
            assert!(validate_condition_flag("unknown", &format!("*{symbol}")).is_err());
        }
    }

    #[test]
    fn capture_sound_guard_exposes_only_its_original_arm_and_keeps_values() {
        use crate::data::{AcmdStmt, ExcuteStmt};
        use crate::move_nodes::{
            model::{MoveCondition, MoveGraph, NodeKind, Track},
            timeline,
        };

        // bayonetta/CapturePulledHi.txt: the else belongs to is_excute in this dump.
        let source = r#"unsafe extern "C" fn sound_capturepulledhi(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_CAPTURE_PULLED_SE) {
        if macros::is_excute(agent) {
            macros::SET_PLAY_INHIVIT(agent, Hash40::new("se_common_cliff_catch"), 5);
        }
        else {
        frame(agent.lua_state_agent, 3.0);
        if macros::is_excute(agent) {
            macros::PLAY_SE(agent, Hash40::new("se_common_cliff_catch"));
        }
    }
}
}"#;
        let script = crate::acmd::parse_sound_script(source);
        let mut imported = timeline::import_for_fighter("bayonetta", Track::Sound, &script);
        let NodeKind::Branch {
            condition,
            then_nodes,
            else_nodes,
        } = &mut imported.nodes[0].kind
        else {
            panic!("capture flag must expose a structured branch");
        };
        assert!(
            matches!(condition, MoveCondition::WorkFlag { symbol, expected: true }
            if symbol == "FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_CAPTURE_PULLED_SE")
        );
        assert!(
            else_nodes.is_empty(),
            "do not re-pair the inner Execute's else"
        );
        let [inner] = then_nodes.as_slice() else {
            panic!("one Execute-owned branch")
        };
        let NodeKind::Branch {
            condition: MoveCondition::IsExecute,
            then_nodes,
            else_nodes,
        } = &inner.kind
        else {
            panic!("the alternate belongs to Execute")
        };
        let NodeKind::BareCommand { command } = &then_nodes[0].kind else {
            panic!("sound remains editable")
        };
        let ExcuteStmt::Sound(call) = command.as_ref() else {
            panic!("one sound command")
        };
        assert_eq!(call.func, "SET_PLAY_INHIVIT");
        assert_eq!(call.sounds, ["se_common_cliff_catch"]);
        assert_eq!(call.tail.as_deref(), Some("5"));
        assert!(matches!(else_nodes[0].kind, NodeKind::Frame { frame: 3.0 }));
        assert!(matches!(&else_nodes[1].kind, NodeKind::Execute { commands }
            if matches!(commands.as_slice(), [ExcuteStmt::Sound(call)] if call.func == "PLAY_SE" && call.sounds == ["se_common_cliff_catch"])));
        let graph = MoveGraph {
            tracks: vec![imported.clone()],
            ..Default::default()
        };
        let restored = serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
        let compiled = timeline::compile("bayonetta", "capturepulledhi", &restored).unwrap();
        assert_eq!(
            serde_json::to_value(&compiled[0].1).unwrap(),
            serde_json::to_value(&script).unwrap()
        );
        let emitted = crate::acmd::preview_sound_fn(&compiled[0].1, "capturepulledhi");
        syn::parse_file(&emitted).unwrap();
        assert_eq!(
            serde_json::to_value(crate::acmd::parse_sound_script(&emitted)).unwrap(),
            serde_json::to_value(&script).unwrap()
        );

        // A changed guard must change the emitted predicate without moving either arm.
        let NodeKind::Branch {
            condition: MoveCondition::WorkFlag { expected, .. },
            ..
        } = &mut imported.nodes[0].kind
        else {
            panic!()
        };
        *expected = false;
        let edited = MoveGraph {
            tracks: vec![imported],
            ..Default::default()
        };
        let compiled = timeline::compile("bayonetta", "capturepulledhi", &edited).unwrap();
        let emitted = crate::acmd::preview_sound_fn(&compiled[0].1, "capturepulledhi");
        assert!(emitted.contains("if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_CAPTURE_PULLED_SE) {"));
        let reparsed = crate::acmd::parse_sound_script(&emitted);
        let [AcmdStmt::RawBlock {
            body: edited_body, ..
        }] = reparsed.stmts.as_slice()
        else {
            panic!()
        };
        let [AcmdStmt::RawBlock {
            body: original_body,
            ..
        }] = script.stmts.as_slice()
        else {
            panic!()
        };
        assert_eq!(
            serde_json::to_value(edited_body).unwrap(),
            serde_json::to_value(original_body).unwrap()
        );
        let unknown = source.replace(
            "DISABLE_CAPTURE_PULLED_SE",
            "DISABLE_CAPTURE_PULLED_SE_UNKNOWN",
        );
        let unknown_script = crate::acmd::parse_sound_script(&unknown);
        let unknown_graph =
            timeline::import_for_fighter("bayonetta", Track::Sound, &unknown_script);
        assert!(crate::move_nodes::test_support::untyped_condition(
            &unknown_graph.nodes[0].kind
        ));
    }

    #[test]
    fn nana_camera_condition_does_not_expand_runtime_or_writable_identifiers() {
        const FLAG: &str = "FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA";
        assert!(read_only_condition_flag_choices("nana").contains(&FLAG));
        assert_eq!(
            validate_condition_flag("nana", &format!("*{FLAG}")).as_deref(),
            Ok(FLAG)
        );
        assert!(registry::fighter_kind_symbol("nana").is_none());
        assert!(validate_flag("nana", &format!("*{FLAG}")).is_err());
        for token in [
            FLAG,
            "*FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERAX",
        ] {
            assert!(validate_condition_flag("nana", token).is_err());
        }
        for unknown in ["nanaex", "pokemon", "unknown"] {
            assert!(read_only_condition_flag_choices(unknown).is_empty());
            assert!(validate_condition_flag(unknown, &format!("*{FLAG}")).is_err());
        }
    }

    #[test]
    fn final_camera_disable_flag_is_read_only_condition_only() {
        use crate::gimmicks::registry::IdentifierKind;

        const SYMBOL: &str = "FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA";
        assert!(read_only_condition_flags().contains(&SYMBOL));
        assert!(!common_flags().contains(&SYMBOL));
        assert!(is_source_only_flag_symbol(SYMBOL));
        assert!(registry::resolve("robot", SYMBOL, IdentifierKind::WorkFlag).is_err());
        assert_eq!(
            validate_condition_flag("robot", &format!("*{SYMBOL}")).as_deref(),
            Ok(SYMBOL)
        );
        assert_eq!(
            validate_condition_flag("lucario", &format!("*{SYMBOL}")).as_deref(),
            Ok(SYMBOL)
        );
        assert!(validate_flag("robot", &format!("*{SYMBOL}")).is_err());
        assert!(validate_condition_flag("unknown_fighter", &format!("*{SYMBOL}")).is_err());
        assert!(validate_condition_flag("robot", SYMBOL).is_err());
        assert!(
            validate_condition_flag("robot", "*FIGHTER_INSTANCE_WORK_ID_FLAG_UNKNOWN").is_err()
        );
    }

    #[test]
    fn fighter_owned_status_flags_are_exact_owner_source_only_entries() {
        use crate::gimmicks::registry::IdentifierKind;

        let entries = super::super::native_status_flags::entries();
        assert_eq!(entries.len(), 864);
        for entry in entries {
            assert!(
                registry::fighter_kind_symbol(entry.fighter).is_some(),
                "{} has no exact pinned fighter kind",
                entry.fighter
            );
            assert_eq!(
                super::super::native_status_flags::resolve(entry.fighter, entry.symbol),
                Some(entry)
            );
            assert_eq!(
                validate_flag(entry.fighter, &format!("*{}", entry.symbol)).as_deref(),
                Ok(entry.symbol)
            );
            assert!(
                registry::resolve(entry.fighter, entry.symbol, IdentifierKind::WorkFlag).is_err(),
                "{} entered the shared runtime work registry",
                entry.symbol
            );
            assert!(is_source_only_flag_symbol(entry.symbol));
            assert!(
                super::super::native_status_flags::choices(entry.fighter)
                    .any(|symbol| symbol == entry.symbol),
                "{} is absent from its owner's closed choices",
                entry.symbol
            );

            for other in entries.iter().map(|candidate| candidate.fighter) {
                if other != entry.fighter && !shared_flag_owners(other).contains(&entry.fighter) {
                    assert!(
                        validate_flag(other, &format!("*{}", entry.symbol)).is_err(),
                        "{} was admitted for foreign owner {other}",
                        entry.symbol
                    );
                }
            }
        }

        for fighter in ["pokemon", "shield", "unknown_fighter"] {
            assert!(validate_flag(fighter, "*FIGHTER_RYU_STATUS_ATTACK_FLAG_HIT_CANCEL").is_err());
        }
        for (fighter, symbol) in [
            (
                "bayonetta",
                "FIGHTER_BAYONETTA_STATUS_WORK_ID_SPECIAL_N_FLAG_SPECIAL_N_CHARGE_MAX",
            ),
            ("pit", "FIGHTER_PIT_STATUS_SPECIAL_N_CHARGE_FLAG_CHARGE_MAX"),
            ("link", "FIGHTER_LINK_STATUS_BOW_FLAG_CHARGE_MAX"),
            (
                "zelda",
                "FIGHTER_ZELDA_STATUS_SPECIAL_N_FLAG_REFLECTOR_START",
            ),
        ] {
            assert_eq!(
                validate_flag(fighter, &format!("*{symbol}")).as_deref(),
                Ok(symbol),
                "semantic boolean flag {symbol} must not be rejected by suffix"
            );
        }
        for symbol in [
            "FIGHTER_BAYONETTA_STATUS_ATTACK_FLAG_START",
            "FIGHTER_BAYONETTA_STATUS_ATTACK_FLAG_TERM",
        ] {
            assert!(
                validate_flag("bayonetta", &format!("*{symbol}")).is_err(),
                "range marker {symbol} must not be admitted as a semantic flag"
            );
        }
        for token in [
            "*FIGHTER_RYU_STATUS_ATTACK_FLAG_HIT_CANCEL + 1",
            "*FIGHTER_RYU_STATUS_KIND_ATTACK",
            "*FIGHTER_RYU_STATUS_WORK_ID_SPECIAL_COMMON_FLAG_COMMAND_EXTRA",
            "FIGHTER_RYU_STATUS_ATTACK_FLAG_HIT_CANCEL",
            "*0x2100000D",
        ] {
            assert!(
                validate_flag("ryu", token).is_err(),
                "accepted invalid status flag token {token:?}"
            );
        }
    }

    #[test]
    fn every_owner_only_flag_is_listed_only_for_its_owner() {
        let entries = super::super::native_status_flags::entries();
        let mut owners = entries
            .iter()
            .map(|entry| entry.fighter)
            .collect::<Vec<_>>();
        owners.sort_unstable();
        owners.dedup();

        for owner in owners {
            let actual = super::super::native_status_flags::choices(owner).collect::<Vec<_>>();
            let expected = entries
                .iter()
                .filter(|entry| entry.fighter == owner)
                .map(|entry| entry.symbol)
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "choices for {owner} crossed owners");
        }
        assert!(super::super::native_status_flags::choices("ken")
            .next()
            .is_none());
        assert!(super::super::native_status_flags::choices("unknown")
            .next()
            .is_none());
    }

    #[test]
    fn added_common_flags_import_preview_and_reparse_exactly_for_concrete_fighters() {
        use crate::data::{AcmdStmt, ExcuteStmt};
        use crate::move_nodes::model::{MoveGraph, NodeKind, Track};

        let added_flags = &common_flags()[11..];
        let mut source = String::from(
            "unsafe extern \"C\" fn game_common_flag_roundtrip(agent: &mut L2CAgentBase) {\n    if macros::is_excute(agent) {\n",
        );
        for &flag in added_flags {
            source.push_str(&format!(
                "        WorkModule::on_flag(agent.module_accessor, *{flag});\n        WorkModule::off_flag(agent.module_accessor, *{flag});\n"
            ));
        }
        source.push_str("    }\n}\n");

        let script = crate::acmd::parse_acmd_script(&source);
        let expected = added_flags.len() * 2;
        assert!(matches!(
            script.stmts.as_slice(),
            [AcmdStmt::Excute(commands)] if commands.len() == expected
                && commands.iter().all(|command| matches!(command, ExcuteStmt::WorkFlag(_)))
        ));

        for fighter in ["mario", "fox", "cloud"] {
            for &flag in added_flags {
                for action in ["on", "off"] {
                    assert_eq!(
                        validate_flag(fighter, &format!("*{flag}")).unwrap(),
                        flag,
                        "{action} flag {flag} was not admitted for {fighter}"
                    );
                }
            }

            let imported =
                crate::move_nodes::timeline::import_for_fighter(fighter, Track::Game, &script);
            assert!(matches!(
                imported.nodes.as_slice(),
                [node] if matches!(
                    &node.kind,
                    NodeKind::Execute { commands }
                        if commands.len() == expected
                            && commands.iter().all(|command| matches!(command, ExcuteStmt::WorkFlag(_)))
                )
            ));

            let mut graph = MoveGraph::default();
            graph.tracks.push(imported);
            assert_eq!(
                crate::move_nodes::live_support::source_only_tracks(&graph).collect::<Vec<_>>(),
                vec![Track::Game],
                "reviewed common flags must not gain flat live writes"
            );
            for &flag in added_flags {
                assert!(
                    registry::resolve(fighter, flag, IdentifierKind::WorkFlag).is_err(),
                    "source-only flag {flag} entered the runtime allowlist"
                );
            }
            let compiled =
                crate::move_nodes::timeline::compile(fighter, "common_flag_roundtrip", &graph)
                    .unwrap_or_else(|errors| {
                        panic!("{fighter} common flags did not compile: {errors:?}")
                    });
            let preview = crate::acmd::preview_game_fn(&compiled[0].1, "common_flag_roundtrip");
            let reparsed = crate::acmd::parse_acmd_script(&preview);
            assert_eq!(
                serde_json::to_value(&reparsed).unwrap(),
                serde_json::to_value(&script).unwrap(),
                "{fighter} common flag preview changed the source statement tree"
            );
        }

        for &flag in added_flags {
            assert!(validate_flag("pokemon", &format!("*{flag}")).is_err());
            assert!(validate_flag("unknown_fighter", &format!("*{flag}")).is_err());
        }
        for token in [
            "*FIGHTER_STATUS_RUN_BRAKE_FLAG_STOP_EXTRA",
            "*FIGHTER_STATUS_RUN_BRAKE_FLAG_STOP + 1",
            "FIGHTER_STATUS_RUN_BRAKE_FLAG_STOP",
        ] {
            assert!(validate_flag("mario", token).is_err(), "accepted {token:?}");
        }
    }

    #[test]
    fn literal_recurring_status_flags_edit_roundtrip_and_leave_runtime_registry_closed() {
        use crate::data::{AcmdStmt, ExcuteStmt, WorkFlagAction};
        use crate::move_nodes::model::{MoveCommand, MoveGraph, NodeKind, Track};

        // Literal spellings and calls observed in the pinned fighter corpus, plus one
        // deliberately unregistered spelling to verify fail-closed preservation.
        const SOURCE: &str = r#"unsafe extern "C" fn game_literal_status_flags(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_AIR_FLAG_LANDING_CLEAR_SPEED);
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_ATTACK_DISABLE_MINI_JUMP_ATTACK);
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_THROW_FLAG_START_AIR);
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACH_WALL_FLAG_MOT_STOP);
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_TURN_RUN_FLAG_STOP);
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_THROW_FLAG_STOP);
        WorkModule::off_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_AIR_FLAG_KEEP_AIR);
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO_INPUT);
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_AIR_LASSO_FLAG_HANG_IMMIDIATE);
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_TURN_RUN_FLAG_TO_RUN);
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_NOT_IN_COMMON_ALLOWLIST);
    }
}
"#;
        let candidates = [
            "FIGHTER_STATUS_ATTACK_AIR_FLAG_LANDING_CLEAR_SPEED",
            "FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_ATTACK_DISABLE_MINI_JUMP_ATTACK",
            "FIGHTER_STATUS_THROW_FLAG_START_AIR",
            "FIGHTER_STATUS_ATTACH_WALL_FLAG_MOT_STOP",
            "FIGHTER_STATUS_TURN_RUN_FLAG_STOP",
            "FIGHTER_STATUS_THROW_FLAG_STOP",
            "FIGHTER_STATUS_ATTACK_AIR_FLAG_KEEP_AIR",
            "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO_INPUT",
            "FIGHTER_STATUS_AIR_LASSO_FLAG_HANG_IMMIDIATE",
            "FIGHTER_STATUS_TURN_RUN_FLAG_TO_RUN",
        ];
        let parsed = crate::acmd::parse_acmd_script(SOURCE);
        assert!(matches!(
            parsed.stmts.as_slice(),
            [AcmdStmt::Excute(commands)] if commands.len() == 11
        ));
        let mut imported =
            crate::move_nodes::timeline::import_for_fighter("mario", Track::Game, &parsed);
        let [node] = imported.nodes.as_slice() else {
            panic!("expected one mixed execute node")
        };
        let NodeKind::MixedExecute { commands } = &node.kind else {
            panic!("unknown source alongside editable flags must remain mixed")
        };
        assert_eq!(commands.len(), 11);
        let mut editable_count = 0;
        let mut preserved_unknown = false;
        for command in commands {
            match command {
                MoveCommand::Editable(ExcuteStmt::WorkFlag(call))
                    if candidates
                        .iter()
                        .any(|candidate| call.flag == format!("*{candidate}")) =>
                {
                    editable_count += 1;
                }
                MoveCommand::Preserved(ExcuteStmt::WorkFlag(call))
                    if call.flag == "*FIGHTER_STATUS_NOT_IN_COMMON_ALLOWLIST" =>
                {
                    preserved_unknown = true;
                }
                _ => {}
            }
        }
        assert_eq!(editable_count, candidates.len());
        assert!(preserved_unknown);

        // Authoring changes both operation directions without opening the source parser or live
        // registry to numeric/unknown aliases.
        let NodeKind::MixedExecute { commands } = &mut imported.nodes[0].kind else {
            unreachable!()
        };
        let mut saw_on = false;
        let mut saw_off = false;
        for command in commands {
            if let MoveCommand::Editable(ExcuteStmt::WorkFlag(call)) = command {
                match call.action {
                    WorkFlagAction::On => {
                        call.action = WorkFlagAction::Off;
                        saw_on = true;
                    }
                    WorkFlagAction::Off => {
                        call.action = WorkFlagAction::On;
                        saw_off = true;
                    }
                }
            }
        }
        assert!(saw_on && saw_off);

        let mut graph = MoveGraph::default();
        graph.tracks.push(imported);
        let saved = serde_json::to_value(&graph).unwrap();
        let restored: MoveGraph = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(serde_json::to_value(&restored).unwrap(), saved);
        assert_eq!(
            crate::move_nodes::live_support::source_only_tracks(&restored).collect::<Vec<_>>(),
            vec![Track::Game]
        );
        for candidate in candidates {
            assert!(
                registry::resolve("mario", candidate, IdentifierKind::WorkFlag).is_err(),
                "source-only flag {candidate} entered the runtime registry"
            );
        }
        assert!(validate_flag("mario", "*FIGHTER_STATUS_NOT_IN_COMMON_ALLOWLIST").is_err());

        let compiled =
            crate::move_nodes::timeline::compile("mario", "literal_status_flags", &restored)
                .unwrap();
        let preview = crate::acmd::preview_game_fn(&compiled[0].1, "literal_status_flags");
        let reparsed = crate::acmd::parse_acmd_script(&preview);
        let [AcmdStmt::Excute(reparsed_commands)] = reparsed.stmts.as_slice() else {
            panic!("edited source did not reparse as one execute statement")
        };
        assert_eq!(reparsed_commands.len(), 11);
        assert!(reparsed_commands.iter().any(|command| matches!(
            command,
            ExcuteStmt::WorkFlag(call)
                if call.flag == "*FIGHTER_STATUS_NOT_IN_COMMON_ALLOWLIST"
        )));
        let edited_on_count = reparsed_commands
            .iter()
            .filter(|command| {
                matches!(
                    command,
                    ExcuteStmt::WorkFlag(call)
                        if call.action == WorkFlagAction::On
                            && candidates.iter().any(|candidate| call.flag == format!("*{candidate}"))
                )
            })
            .count();
        let edited_off_count = reparsed_commands
            .iter()
            .filter(|command| {
                matches!(
                    command,
                    ExcuteStmt::WorkFlag(call)
                        if call.action == WorkFlagAction::Off
                            && candidates.iter().any(|candidate| call.flag == format!("*{candidate}"))
                )
            })
            .count();
        assert_eq!(edited_on_count, 1);
        assert_eq!(edited_off_count, candidates.len() - 1);
    }

    #[test]
    fn common_flags_reject_unknown_names_and_spelling_variants() {
        for token in [
            "*FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD_EXTRA",
            "*FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_CHANGE_STATUS_DELAY_MOTION",
            "*FIGHTER_STATUS_ATTACK_FLAG_100_CONTINUE_CHECKED",
            "*FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SCOPE_SHOOT",
            "*FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_SHOOT_EXTRA",
            "*FIGHTER_MARIO_STATUS_ATTACK_FLAG_START_SMASH_HOLD",
        ] {
            assert!(validate_flag("mario", token).is_err(), "accepted {token:?}");
        }
    }

    #[test]
    fn malformed_wrong_kind_and_fighter_owned_cross_owner_names_reject() {
        for token in [
            "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO",
            "*",
            "*0x2100000D",
            "*FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO + 1",
            "**FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO",
            "*FIGHTER_STATUS_KIND_WAIT",
        ] {
            assert!(validate_flag("mario", token).is_err(), "accepted {token:?}");
        }
        assert!(
            validate_flag("mario", "*FIGHTER_CLOUD_INSTANCE_WORK_ID_FLAG_LIMIT_BREAK").is_err()
        );
        assert_eq!(
            validate_flag("cloud", "*FIGHTER_CLOUD_INSTANCE_WORK_ID_FLAG_LIMIT_BREAK").unwrap(),
            "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLAG_LIMIT_BREAK"
        );
    }

    #[test]
    fn six_added_flags_import_and_compile_from_cached_game_scripts() {
        use crate::data::{AcmdStmt, ExcuteStmt};
        use crate::move_nodes::model::{MoveGraph, Track};

        let cache = crate::scratch_dirs::app_storage_root().join("script-cache");
        let Ok(fighters) = std::fs::read_dir(&cache) else {
            return;
        };
        let mut direct = [0usize; 6];
        let mut opaque_or_loop = [0usize; 6];
        let mut bare = [0usize; 6];
        let mut editable = [0usize; 6];
        let mut preserved = [0usize; 6];
        let mut script_count = 0usize;
        let mut roundtripped = 0usize;
        let mut problems = Vec::new();
        let mut preserved_scope = false;

        for fighter_entry in fighters.flatten() {
            let fighter_path = fighter_entry.path();
            if !fighter_path.is_dir() {
                continue;
            }
            let Some(fighter) = fighter_path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Ok(files) = std::fs::read_dir(&fighter_path) else {
                continue;
            };
            for file_entry in files.flatten() {
                let path = file_entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("txt") {
                    continue;
                }
                let Ok(source) = std::fs::read_to_string(&path) else {
                    continue;
                };
                if crate::acmd::extract_function(&source, "game_").is_none() {
                    continue;
                }
                let script = crate::acmd::parse_acmd_script(&source);
                let mut script_direct = [0usize; 6];
                let mut script_nested = [0usize; 6];
                let mut script_bare = [0usize; 6];
                count_script_flags(
                    &script.stmts,
                    &mut script_direct,
                    &mut script_nested,
                    &mut script_bare,
                    false,
                );
                for index in 0..6 {
                    direct[index] += script_direct[index];
                    opaque_or_loop[index] += script_nested[index];
                    bare[index] += script_bare[index];
                }

                if script.stmts.iter().any(|stmt| {
                    matches!(
                        stmt,
                        AcmdStmt::RawBlock { header, .. }
                            if header.contains("FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_SHOOT")
                    )
                }) {
                    let has_scope_call = script.stmts.iter().any(|stmt| match stmt {
                        AcmdStmt::RawBlock { body, .. } => body.iter().any(|nested| matches!(
                            nested,
                            AcmdStmt::Excute(commands) if commands.iter().any(|command| matches!(
                                command,
                                ExcuteStmt::WorkFlag(call)
                                    if call.flag == "*FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_SHOOT"
                            ))
                        )),
                        _ => false,
                    });
                    if has_scope_call {
                        let imported = crate::move_nodes::timeline::import_for_fighter(
                            fighter,
                            Track::Game,
                            &script,
                        );
                        preserved_scope |= has_scoped_scope_condition(&imported.nodes);
                        let mut graph = MoveGraph::default();
                        graph.tracks.push(imported);
                        match crate::move_nodes::timeline::compile(
                            fighter,
                            "work_flag_scope_preservation",
                            &graph,
                        ) {
                            Ok(compiled)
                                if serde_json::to_value(&compiled[0].1).ok()
                                    == serde_json::to_value(&script).ok() => {}
                            Ok(_) => problems.push(format!(
                                "{}: scoped SSCOPE source changed after round-trip",
                                path.display()
                            )),
                            Err(errors) => problems.push(format!(
                                "{}: scoped SSCOPE source did not compile: {errors:?}",
                                path.display()
                            )),
                        }
                    }
                }

                if script_direct.iter().sum::<usize>() == 0 {
                    continue;
                }
                script_count += 1;
                let imported =
                    crate::move_nodes::timeline::import_for_fighter(fighter, Track::Game, &script);
                count_editable_flags(&imported.nodes, &mut editable, &mut preserved);
                let mut graph = MoveGraph::default();
                graph.tracks.push(imported);
                match crate::move_nodes::timeline::compile(
                    fighter,
                    "work_flag_source_roundtrip",
                    &graph,
                ) {
                    Ok(compiled)
                        if serde_json::to_value(&compiled[0].1).ok()
                            == serde_json::to_value(&script).ok() =>
                    {
                        roundtripped += 1
                    }
                    Ok(_) => problems.push(format!(
                        "{}: statement tree changed on export",
                        path.display()
                    )),
                    Err(errors) => problems.push(format!(
                        "{}: imported source did not compile: {errors:?}",
                        path.display()
                    )),
                }
            }
        }

        if script_count == 0 {
            return;
        }
        assert_eq!(
            direct,
            [10, 10, 9, 7, 5, 5],
            "measured direct cache counts changed"
        );
        assert_eq!(direct.iter().sum::<usize>(), 46);
        assert_eq!(
            opaque_or_loop, [0; 6],
            "target flag entered opaque control flow"
        );
        assert_eq!(bare, [0; 6], "target flag appeared as a bare statement");
        assert_eq!(editable, direct, "a direct target flag stayed immutable");
        assert_eq!(
            preserved, [0; 6],
            "a direct target flag was preserved instead"
        );
        assert_eq!(
            roundtripped, script_count,
            "not every target script roundtripped"
        );
        assert!(
            preserved_scope,
            "SSCOPE flag actions lost their enclosing native condition"
        );
        assert!(
            problems.is_empty(),
            "{} source round-trip failures:\n{}",
            problems.len(),
            problems.join("\n")
        );
    }
}

#[cfg(test)]
mod owned_condition_tests {
    use super::*;

    #[test]
    fn condition_only_owner_pairs_keep_existing_read_and_write_boundaries() {
        for (owner, flags) in OWNED_CONDITION_FLAGS {
            for symbol in *flags {
                let token = format!("*{symbol}");
                assert_eq!(
                    validate_condition_flag(owner, &token).as_deref(),
                    Ok(*symbol)
                );
                assert!(read_only_condition_flag_choices(owner).contains(symbol));
                assert!(validate_flag(owner, &token).is_err());
                for other in ["mario", "unknown"] {
                    assert!(validate_condition_flag(other, &token).is_err());
                }
                assert!(validate_condition_flag(owner, &format!("{token}_UNKNOWN")).is_err());
                // A condition-only alias cannot revoke the original owner's existing read.
                for original in ["fox", "element", "sonic"] {
                    if validate_flag(original, &token).is_ok() {
                        assert!(validate_condition_flag(original, &token).is_ok());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod corpus_condition_read_tests {
    use super::*;
    #[test]
    fn corpus_weapon_flags_are_read_only_and_owner_checked() {
        // Vanilla tantan_ring/AttackFly.txt.
        let symbol = "WEAPON_TANTAN_RING_INSTANCE_WORK_ID_FLAG_IS_AIR";
        let token = format!("*{symbol}");
        assert_eq!(
            validate_condition_flag("tantan_ring", &token).as_deref(),
            Ok(symbol)
        );
        assert!(validate_condition_flag("mario", &token).is_err());
        assert!(validate_condition_flag("tantan", &token).is_err());
        assert!(validate_flag("tantan_ring", &token).is_err());
        assert!(validate_condition_flag("tantan_ring", &format!("{token}_UNKNOWN")).is_err());
    }
}
