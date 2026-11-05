use serde::{Deserialize, Serialize};

#[path = "native_common_calls.rs"]
mod common;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeCall {
    pub callee: String,
    pub arguments: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_line: Option<SourceLine>,
}

/// A decompiler statement whose operands do not satisfy the pinned SDK ABI.
/// Kept separately from callable arguments: no missing value is inferred or editable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLine {
    source: String,
}

// Total SDK argument counts, including the module accessor. These dump spellings
// cannot be emitted as real SDK calls without recovering the omitted stack values.
const SDK_ARITIES: &[(&str, usize)] = &[
    ("ItemModule::set_have_item_action", 4),
    ("ItemModule::set_have_item_scale_anim", 4),
    ("ItemModule::pickup_item", 6),
    ("ItemModule::use_item", 3),
    ("ItemModule::set_attach_item_visibility", 3),
];

impl SourceLine {
    fn sdk_arity(&self) -> Option<(usize, usize)> {
        let source = self.source.as_str();
        if source.len() > 2048 || source.trim() != source || source.contains('\n') {
            return None;
        }
        // Exactly one unattributed statement. A call embedded in a condition,
        // assignment, second statement or macro body is never a source-line match.
        let syn::Stmt::Expr(syn::Expr::Call(call), Some(_)) =
            syn::parse_str::<syn::Stmt>(source).ok()?
        else {
            return None;
        };
        if !call.attrs.is_empty() {
            return None;
        }
        let (callee, args) = source.strip_suffix(");")?.split_once('(')?;
        let expected = SDK_ARITIES.iter().find(|(name, _)| *name == callee)?.1;
        let args = crate::acmd::tokenize_args_balanced(args);
        if args.len() != call.args.len()
            || args.len() >= expected
            || !matches!(args.first()?.as_str(), "agent.module_accessor" | "boma")
            || args.len() < 2
        {
            return None;
        }
        // Opaque operands are still bounded data, never arbitrary executable Rust.
        if !args[1..].iter().all(|value| {
            Number.accepts(value)
                || Bool.accepts(value)
                || syn::parse_str::<syn::Expr>(value)
                    .ok()
                    .and_then(|expr| constant_symbol(&expr))
                    .is_some()
        }) {
            return None;
        }
        Some((expected, args.len()))
    }
}

const SOURCE_LINE_SIGNATURE: Signature = signature("", "Opaque source line", "", &[]);

#[derive(Clone, Copy)]
pub enum ArgumentKind {
    Number,
    Float(bool),
    HashInteger,
    Byte(&'static [&'static str]),
    Unsigned,
    Vector3,
    SimdVector(usize),
    ScreenLayer,
    Article,
    CliffProfile,
    Exact(&'static str),
    Work(
        crate::gimmicks::registry::IdentifierKind,
        &'static [&'static str],
    ),
    Integer(&'static [&'static str]),
    Bool,
    Hash,
}

pub struct Signature {
    pub callee: &'static str,
    pub caption: &'static str,
    pub receiver: &'static str,
    pub arguments: &'static [(&'static str, ArgumentKind)],
    pub stack_wrapper: bool,
    pub bindable: bool,
}

use ArgumentKind::*;
pub const SIGNATURES: &[Signature] = &[
    // Pinned skyline-smash cpp.rs: accessor, attack index, float. (set_size stays on the closed AttackModuleSizePreset path in acmd.rs.)
    signature(
        "AttackModule::set_ink_value",
        "Set attack ink",
        "agent.module_accessor",
        &[("Attack ID", Integer(&[])), ("Ink", Float(true))],
    ),
    // skyline-smash cpp.rs: accessor, c_int, bool, bool. Keep all operands editable.
    signature(
        "AttackModule::set_force_reaction",
        "Set force reaction",
        "agent.module_accessor",
        &[
            ("Attack ID", Integer(&[])),
            ("Enable", Bool),
            ("Argument 4", Bool),
        ],
    ),
    signature(
        "macros::ATK_SET_SHIELD_SETOFF_MUL_arg4",
        "Shield setoff multiplier (three attacks)",
        "agent",
        &[
            ("Attack ID 1", Unsigned),
            ("Attack ID 2", Unsigned),
            ("Attack ID 3", Unsigned),
            ("Multiplier", Float(true)),
        ],
    ),
    signature(
        "macros::ATK_SET_SHIELD_SETOFF_MUL_arg5",
        "Shield setoff multiplier (four attacks)",
        "agent",
        &[
            ("Attack ID 1", Unsigned),
            ("Attack ID 2", Unsigned),
            ("Attack ID 3", Unsigned),
            ("Attack ID 4", Unsigned),
            ("Multiplier", Float(true)),
        ],
    ),
    signature(
        "macros::FILL_SCREEN_MODEL_COLOR",
        "Fill screen model color",
        "agent",
        &[
            ("Index", Integer(&[])),
            ("Duration", Number),
            ("Multiply red", Number),
            ("Multiply green", Number),
            ("Multiply blue", Number),
            ("Add red", Number),
            ("Add green", Number),
            ("Add blue", Number),
            ("Blend", Number),
            ("Argument 10", Number),
            ("Screen layer", ScreenLayer),
            (
                "Priority",
                Integer(&[
                    "EFFECT_SCREEN_PRIO_FINAL",
                    "EFFECT_SCREEN_PRIO_ITEM",
                    "EFFECT_SCREEN_PRIO_NORMAL",
                ]),
            ),
        ],
    ),
    signature(
        "EffectModule::request_post_effect_line_circle",
        "Request circle post effect",
        "agent.module_accessor",
        &[
            ("Effect", Hash),
            ("Bone", Hash),
            ("Radius", SimdVector(2)),
            ("Offset", SimdVector(3)),
            ("Flag", Bool),
            ("Duration", Float(true)),
            ("Fade", Float(true)),
        ],
    ),
    signature(
        "EffectModule::remove_post_effect_line",
        "Remove line post effect",
        "agent.module_accessor",
        &[("Index", Integer(&[])), ("Immediate", Bool)],
    ),
    // SDK stack entry points; light effects have their own 17-operand layout.
    Signature {
        stack_wrapper: true,
        ..signature(
            "macros::RUMBLE_HIT_STATUS",
            "Hit status rumble",
            "agent",
            &[("Rumble", Hash), ("Duration", Integer(&[]))],
        )
    },
    Signature {
        stack_wrapper: true,
        ..signature(
            "macros::EFFECT_FOLLOW_LIGHT",
            "Follow light effect",
            "agent",
            &[
                ("Effect", Hash),
                ("Bone", Hash),
                ("X", Number),
                ("Y", Number),
                ("Z", Number),
                ("Rotation X", Number),
                ("Rotation Y", Number),
                ("Rotation Z", Number),
                ("Scale", Number),
                ("Flag", Bool),
                ("Light operand 1", Number),
                ("Light operand 2", Number),
                ("Light operand 3", Number),
                ("Light operand 4", Number),
                ("Light operand 5", Number),
                ("Light operand 6", Number),
                ("Light operand 7", Number),
            ],
        )
    },
    signature(
        "AttackModule::enable_safe_pos",
        "Enable safe position",
        "agent.module_accessor",
        &[],
    ),
    signature(
        "AttackModule::disable_tip",
        "Disable attack tip",
        "agent.module_accessor",
        &[],
    ),
    signature(
        "AttackModule::set_attack_keep_rumble",
        "Keep attack rumble",
        "agent.module_accessor",
        &[("Attack ID", Integer(&[])), ("Keep", Bool)],
    ),
    signature(
        "GroundModule::select_cliff_hangdata",
        "Cliff hang profile",
        "agent.module_accessor",
        &[("Profile", CliffProfile)],
    ),
    signature(
        "ArticleModule::set_visibility",
        "Article part visibility",
        "agent.module_accessor",
        &[
            ("Article", Article),
            ("Group", Hash),
            ("Variant", Hash),
            (
                "Target",
                Exact("ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL)"),
            ),
        ],
    ),
    // Match the pinned skyline-smash StatusModule scripted fighter example.
    signature(
        "StatusModule::change_status_request_from_script",
        "Change status",
        "agent.module_accessor",
        &[
            (
                "Status",
                Work(
                    crate::gimmicks::registry::IdentifierKind::FighterStatus,
                    &[],
                ),
            ),
            ("Script request", Exact("true")),
        ],
    ),
    signature(
        "macros::THROW_ITEM_arg3",
        "Throw item",
        "agent",
        &[
            (
                "Angle slot",
                Integer(&["ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_ANGLE"]),
            ),
            (
                "Speed slot",
                Integer(&["ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_SPEED"]),
            ),
            (
                "Power slot",
                Integer(&["ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_POWER"]),
            ),
        ],
    ),
    signature(
        "macros::REQ_FINAL_START_CAMERA_arg3",
        "Final camera animation",
        "agent",
        &[("Animation", Hash), ("Flag 1", Bool), ("Flag 2", Bool)],
    ),
    signature(
        "macros::REQ_FINAL_START_CAMERA",
        "Final camera animation",
        "agent",
        &[("Animation", Hash), ("Flag", Bool)],
    ),
    signature(
        "macros::FT_ADD_DAMAGE",
        "Add damage",
        "agent",
        &[("Damage", Number)],
    ),
    signature(
        "macros::FT_LEAVE_NEAR_OTTOTTO",
        "Cliff distance",
        "agent",
        &[("Distance 1", Number), ("Distance 2", Number)],
    ),
    signature(
        "macros::COL_PRI",
        "Color priority",
        "agent",
        &[("Priority", Unsigned)],
    ),
    signature(
        "macros::QUAKE",
        "Camera quake",
        "agent",
        &[(
            "Quake",
            Integer(&[
                "CAMERA_QUAKE_KIND_NONE",
                "CAMERA_QUAKE_KIND_S",
                "CAMERA_QUAKE_KIND_S_HALF",
                "CAMERA_QUAKE_KIND_M",
                "CAMERA_QUAKE_KIND_L",
                "CAMERA_QUAKE_KIND_XL",
            ]),
        )],
    ),
    signature(
        "macros::ATK_SET_SHIELD_SETOFF_MUL",
        "Shield setoff multiplier",
        "agent",
        &[("Attack ID", Unsigned), ("Multiplier", Float(true))],
    ),
    Signature {
        stack_wrapper: true,
        ..signature(
            "macros::EFFECT_FOLLOW_RND",
            "Follow effect with random ranges",
            "agent",
            &[
                ("Effect", Hash),
                ("Bone", Hash),
                ("X", Number),
                ("Y", Number),
                ("Z", Number),
                ("Rotation X", Number),
                ("Rotation Y", Number),
                ("Rotation Z", Number),
                ("Scale", Number),
                ("X range", Number),
                ("Y range", Number),
                ("Z range", Number),
                ("Rotation X range", Number),
                ("Rotation Y range", Number),
                ("Rotation Z range", Number),
                ("Flag", Bool),
            ],
        )
    },
    signature(
        "shield!",
        "Native shield state",
        "agent",
        &[
            (
                "Command",
                Integer(&["MA_MSC_CMD_SHIELD_ON", "MA_MSC_CMD_SHIELD_OFF"]),
            ),
            (
                "Kind",
                Integer(&["COLLISION_KIND_SHIELD", "COLLISION_KIND_REFLECTOR"]),
            ),
            (
                "ID",
                Integer(&[
                    "FIGHTER_PEACH_SHIELD_KIND_KINOPIO_GUARD",
                    "FIGHTER_LUCARIO_SHIELD_KIND_SPLIT",
                    "FIGHTER_MARIO_REFLECTOR_KIND_MANTLE",
                    "FIGHTER_MARIOD_REFLECTOR_KIND_DRMANTLE",
                    "FIGHTER_PIT_REFLECTOR_KIND_SPECIAL_S",
                    "FIGHTER_PALUTENA_REFLECTOR_KIND_REFLECTOR",
                    "FIGHTER_JACK_REFLECTOR_KIND_SPECIAL_LW",
                    "FIGHTER_MIISWORDSMAN_REFLECTOR_KIND_REVERSE_SLASH",
                    "FIGHTER_ROBOT_REFLECTOR_KIND_ARMSPIN",
                ]),
            ),
            (
                "Group",
                Integer(&[
                    "FIGHTER_REFLECTOR_GROUP_EXTEND",
                    "FIGHTER_PEACH_SHIELD_GROUP_KIND_KINOPIO_GUARD",
                    "FIGHTER_LUCARIO_SHIELD_GROUP_KIND_SPLIT",
                    "FIGHTER_PIT_REFLECTOR_GROUP_SPECIAL_S",
                    "FIGHTER_PIT_SHIELD_GROUP_KIND_SPECIAL_LW",
                    "FIGHTER_PIT_REFLECTOR_GROUP_SPECIAL_LW",
                    "FIGHTER_EDGE_SHIELD_GROUP_KIND_SPECIAL_LW",
                    "FIGHTER_JACK_SHIELD_GROUP_KIND_SPECIAL_LW",
                ]),
            ),
        ],
    ),
    signature(
        "AttackModule::set_attack_level",
        "Attack level",
        "agent.module_accessor",
        &[
            ("Attack ID", Integer(&[])),
            (
                "Level",
                Byte(&[
                    "FIGHTER_RYU_SAVING_LV_1",
                    "FIGHTER_RYU_SAVING_LV_2",
                    "FIGHTER_RYU_SAVING_LV_3",
                ]),
            ),
        ],
    ),
    signature(
        "WorkModule::on_flag",
        "Set native flag",
        "agent.module_accessor",
        &[(
            "Flag",
            Work(crate::gimmicks::registry::IdentifierKind::WorkFlag, &[]),
        )],
    ),
    signature(
        "WorkModule::off_flag",
        "Clear native flag",
        "agent.module_accessor",
        &[(
            "Flag",
            Work(crate::gimmicks::registry::IdentifierKind::WorkFlag, &[]),
        )],
    ),
    Signature {
        callee: "macros::FLASH_SET_DIRECTION",
        caption: "Flash direction",
        receiver: "agent",
        arguments: &[("X", Number), ("Y", Number), ("Z", Number)],
        stack_wrapper: true,
        bindable: false,
    },
    Signature {
        callee: "macros::FT_REMOVE_FINAL_AURA",
        caption: "Remove final aura",
        receiver: "agent",
        arguments: &[("Enabled", Bool)],
        stack_wrapper: true,
        bindable: false,
    },
    signature(
        "physics!",
        "Charge physics",
        "agent",
        &[
            ("Command", Exact("*MA_MSC_CMD_PHYSICS_START_CHARGE")),
            ("Argument 1", Number),
            ("Argument 2", Number),
            ("Argument 3", Number),
            ("Argument 4", Number),
            ("Argument 5", Number),
            ("Argument 6", Number),
            ("Bone", Hash),
        ],
    ),
    signature(
        "physics!",
        "Stop swing physics",
        "agent",
        &[
            ("Command", Exact("*MA_MSC_CMD_PHYSICS_PH_SWING_STOP")),
            ("Stopped", Bool),
        ],
    ),
    signature(
        "camera!",
        "Reset camera",
        "agent",
        &[("Command", Exact("*MA_MSC_CMD_CAMERA_CAM_RESET"))],
    ),
    signature(
        "camera!",
        "Camera offset",
        "agent",
        &[
            ("Command", Exact("*MA_MSC_CMD_CAMERA_CAM_OFFSET")),
            ("X", Number),
            ("Y", Number),
        ],
    ),
    signature(
        "camera!",
        "Camera offset",
        "agent",
        &[
            ("Command", Exact("*MA_MSC_CMD_CAMERA_CAM_OFFSET")),
            ("X", Number),
            ("Y", Number),
            ("Z", Number),
        ],
    ),
    signature(
        "camera!",
        "Camera bounds",
        "agent",
        &[
            ("Command", Exact("*MA_MSC_CMD_CAMERA_CAM_RECT")),
            ("Right", Number),
            ("Left", Number),
            ("Top", Number),
            ("Bottom", Number),
        ],
    ),
    signature(
        "camera!",
        "Camera position updates",
        "agent",
        &[
            ("Command", Exact("*MA_MSC_CMD_CAMERA_SET_UPDATE_POS")),
            ("Axes", Integer(&["CAMERA_UPDATE_POS_XYZ"])),
        ],
    ),
    signature(
        "search!",
        "Clear searches",
        "agent",
        &[("Command", Exact("*MA_MSC_CMD_SEARCH_SEARCH_SCH_CLR_ALL"))],
    ),
    signature(
        "search!",
        "Clear search",
        "agent",
        &[
            ("Command", Exact("*MA_MSC_CMD_SEARCH_SEARCH_SCH_CLR")),
            ("Search ID", Integer(&[])),
        ],
    ),
    signature(
        "search!",
        "Search opponent properties",
        "agent",
        &[
            ("Command", Exact("*MA_MSC_CMD_SEARCH_SET_OPPONENT")),
            ("Search ID", Integer(&[])),
            ("Argument 2", Integer(&[])),
            ("Property", Exact("*COLLISION_TARGET_PROPERTY")),
            ("Mask", Exact("*COLLISION_PROPERTY_MASK_REFLECT")),
        ],
    ),
    signature(
        "shield!",
        "Reflector",
        "agent",
        &[
            ("Command", Exact("*MA_MSC_CMD_REFLECTOR")),
            ("Kind", Exact("*COLLISION_KIND_REFLECTOR")),
            ("Reflector ID", Integer(&[])),
            ("Bone", Hash),
            ("Size", Number),
            ("X", Number),
            ("Y", Number),
            ("Z", Number),
            ("X2", Number),
            ("Y2", Number),
            ("Z2", Number),
            ("Damage multiplier", Number),
            ("Speed multiplier", Number),
            ("Maximum damage", Number),
            ("Flag", Bool),
            ("Life multiplier", Number),
            ("Group", Integer(&["FIGHTER_REFLECTOR_GROUP_HOMERUNBAT"])),
        ],
    ),
    Signature {
        callee: "macros::THROW_ITEM_OFFSET",
        caption: "Throw item with offset",
        receiver: "agent",
        arguments: &[
            ("Offset X", Number),
            ("Offset Y", Number),
            (
                "Angle slot",
                Integer(&["ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_ANGLE"]),
            ),
            (
                "Speed slot",
                Integer(&["ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_SPEED"]),
            ),
            (
                "Power slot",
                Integer(&["ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_POWER"]),
            ),
        ],
        stack_wrapper: true,
        bindable: false,
    },
    Signature {
        callee: "macros::SHOOT_ITEM_BULLET_CHARGE",
        caption: "Fire charged item",
        receiver: "agent",
        arguments: &[(
            "Charge",
            Integer(&[
                "FIGHTER_STATUS_ITEM_SHOOT_WORK_INT_SSCOPE_CHARGE",
                "FIGHTER_STATUS_ITEM_SWING_WORK_INT_S4_HOLD_FRAME",
            ]),
        )],
        stack_wrapper: true,
        bindable: false,
    },
    Signature {
        callee: "macros::FT_MOTION_INTP_WAIT_ITEM",
        caption: "Item motion interpolation",
        receiver: "agent",
        arguments: &[("Enabled", Bool), ("Argument 2", Number)],
        stack_wrapper: true,
        bindable: false,
    },
    Signature {
        callee: "macros::RUMBLE_FINAL_TARGET",
        caption: "Rumble final target",
        receiver: "agent",
        arguments: &[("Rumble", Hash), ("Argument 2", Integer(&[]))],
        stack_wrapper: true,
        bindable: false,
    },
    signature("macros::SHOOT_ITEM_BULLET", "Fire item", "agent", &[]),
    signature("macros::FT_START_CUTIN", "Start cut-in", "agent", &[]),
    signature(
        "macros::LAST_EFFECT_SET_COLOR",
        "Effect color",
        "agent",
        &[("Red", Number), ("Green", Number), ("Blue", Number)],
    ),
    signature(
        "macros::LAST_PARTICLE_SET_COLOR",
        "Particle color",
        "agent",
        &[("Red", Number), ("Green", Number), ("Blue", Number)],
    ),
    signature(
        "macros::LAST_EFFECT_SET_RATE",
        "Effect rate",
        "agent",
        &[("Rate", Number)],
    ),
    signature(
        "macros::FT_SHOOTING_ATTACK_GROUND_CHECK_NEW",
        "Shooting ground check",
        "agent",
        &[
            ("Argument 1", Number),
            ("Argument 2", Number),
            ("Argument 3", Number),
        ],
    ),
    signature(
        "macros::ATK_SET_SHIELD_SETOFF_MUL_arg3",
        "Shield setoff multiplier",
        "agent",
        &[
            ("Attack ID", Unsigned),
            ("Argument 2", Number),
            ("Multiplier", Float(true)),
        ],
    ),
    signature(
        "lua_bind::FighterCutInManager::set_throw_finish_zoom_rate",
        "Throw camera zoom",
        "singletons::FighterCutInManager()",
        &[("Zoom", Float(true))],
    ),
    signature(
        "lua_bind::FighterCutInManager::set_throw_finish_offset",
        "Throw camera offset",
        "singletons::FighterCutInManager()",
        &[("Offset", Vector3)],
    ),
    signature(
        "FighterSpecializer_Demon::set_devil",
        "Devil form",
        "agent.module_accessor",
        &[("Enabled", Bool), ("Argument 2", Float(true))],
    ),
    signature(
        "ArticleModule::change_motion",
        "Article motion",
        "agent.module_accessor",
        &[
            ("Article", Article),
            ("Motion", Hash),
            ("Argument 3", Bool),
            ("Frame", Float(false)),
        ],
    ),
    signature(
        "PostureModule::scale",
        "Read model scale",
        "agent.module_accessor",
        &[],
    ),
    signature(
        "notify_event_msc_cmd!",
        "Notify event",
        "agent",
        &[
            ("Event", Exact("Hash40::new_raw(0x1f20a9d549)")),
            ("Enabled", Bool),
        ],
    ),
    signature(
        "notify_event_msc_cmd!",
        "Notify event",
        "agent",
        &[
            ("Event", Exact("Hash40::new_raw(0x24772eddef)")),
            ("Enabled", Bool),
        ],
    ),
    signature(
        "notify_event_msc_cmd!",
        "Notify event",
        "agent",
        &[
            ("Event", Exact("Hash40::new_raw(0x2ea0f68425)")),
            ("Enabled", Bool),
        ],
    ),
    signature(
        "notify_event_msc_cmd!",
        "Notify event",
        "agent",
        &[
            ("Event", Exact("Hash40::new_raw(0x2bfb02b69a)")),
            ("Enabled", Bool),
        ],
    ),
    signature(
        "notify_event_msc_cmd!",
        "Notify event",
        "agent",
        &[
            ("Event", Exact("Hash40::new_raw(0x28f4e20a43)")),
            ("Enabled", Bool),
        ],
    ),
    signature(
        "notify_event_msc_cmd!",
        "Notify event",
        "agent",
        &[("Event", Exact("Hash40::new_raw(0x23c33f3bdc)"))],
    ),
    signature(
        "notify_event_msc_cmd!",
        "Notify event",
        "agent",
        &[("Event", Exact("Hash40::new_raw(0x25813802b6)"))],
    ),
    signature(
        "notify_event_msc_cmd!",
        "Notify event",
        "agent",
        &[("Event", Exact("Hash40::new_raw(0x1bcd6668a0)"))],
    ),
    signature(
        "notify_event_msc_cmd!",
        "Notify event",
        "agent",
        &[
            ("Event", Exact("Hash40::new_raw(0x2127e37c07)")),
            (
                "Cliff check",
                Integer(&[
                    "GROUND_CLIFF_CHECK_KIND_NONE",
                    "GROUND_CLIFF_CHECK_KIND_ALWAYS",
                    "GROUND_CLIFF_CHECK_KIND_ALWAYS_BOTH_SIDES",
                    "GROUND_CLIFF_CHECK_KIND_ON_DROP",
                    "GROUND_CLIFF_CHECK_KIND_ON_DROP_BOTH_SIDES",
                    "GROUND_CLIFF_CHECK_KIND_ALWAYS_BOTH_SIDES_NEAR",
                ]),
            ),
        ],
    ),
    signature(
        "notify_event_msc_cmd!",
        "Notify event",
        "agent",
        &[
            ("Event", Exact("Hash40::new_raw(0x26769bd1de)")),
            ("Argument 1", Number),
            ("Argument 2", Number),
            ("Argument 3", Number),
        ],
    ),
    signature(
        "notify_event_msc_cmd!",
        "Notify event",
        "agent",
        &[
            ("Event", Exact("Hash40::new_raw(0x25fd66ecef)")),
            ("Argument 1", Number),
            ("Argument 2", Number),
            ("Argument 3", Number),
        ],
    ),
    signature(
        "notify_event_msc_cmd!",
        "Notify event",
        "agent",
        &[
            ("Event", Exact("Hash40::new_raw(0x1f5b14bb65)")),
            (
                "Arm",
                Integer(&["FIGHTER_ROCKMAN_ARM_LEFT", "FIGHTER_ROCKMAN_ARM_RIGHT"]),
            ),
            (
                "Form",
                Integer(&[
                    "FIGHTER_ROCKMAN_ARMFORM_HAND",
                    "FIGHTER_ROCKMAN_ARMFORM_ROCKBUSTER",
                    "FIGHTER_ROCKMAN_ARMFORM_WASTEHEAT",
                ]),
            ),
            ("Argument 1", Number),
        ],
    ),
    signature(
        "notify_event_msc_cmd!",
        "Notify event",
        "agent",
        &[
            ("Event", Exact("Hash40::new_raw(0x1f5b14bb65)")),
            (
                "Arm",
                Integer(&["FIGHTER_ROCKMAN_ARM_LEFT", "FIGHTER_ROCKMAN_ARM_RIGHT"]),
            ),
            (
                "Form",
                Integer(&[
                    "FIGHTER_ROCKMAN_ARMFORM_HAND",
                    "FIGHTER_ROCKMAN_ARMFORM_ROCKBUSTER",
                    "FIGHTER_ROCKMAN_ARMFORM_WASTEHEAT",
                ]),
            ),
            ("Argument 1", Number),
            ("Argument 2", Number),
        ],
    ),
    signature(
        "notify_event_msc_cmd!",
        "Notify event",
        "agent",
        &[
            ("Event", Exact("Hash40::new_raw(0x2b7cb92b79)")),
            (
                "Shooting slot",
                Integer(&[
                    "FIGHTER_BAYONETTA_SHOOTING_SLOT_L_ARM",
                    "FIGHTER_BAYONETTA_SHOOTING_SLOT_R_ARM",
                    "FIGHTER_BAYONETTA_SHOOTING_SLOT_L_LEG",
                    "FIGHTER_BAYONETTA_SHOOTING_SLOT_R_LEG",
                ]),
            ),
            ("Flag 1", Bool),
            ("Flag 2", Bool),
            ("Flag 3", Bool),
            ("Argument 1", Number),
        ],
    ),
    signature(
        "notify_event_msc_cmd!",
        "Notify event",
        "agent",
        &[
            ("Event", Exact("Hash40::new_raw(0x2d51fcdb09)")),
            (
                "Shooting slot",
                Integer(&[
                    "FIGHTER_BAYONETTA_SHOOTING_SLOT_L_ARM",
                    "FIGHTER_BAYONETTA_SHOOTING_SLOT_R_ARM",
                    "FIGHTER_BAYONETTA_SHOOTING_SLOT_L_LEG",
                    "FIGHTER_BAYONETTA_SHOOTING_SLOT_R_LEG",
                ]),
            ),
            ("Flag 1", Bool),
            ("Flag 2", Bool),
            ("Flag 3", Bool),
            ("Argument 1", Number),
            ("Argument 2", Number),
            ("Argument 3", Number),
            ("Argument 4", Number),
            ("Flag 4", Bool),
        ],
    ),
    // smash-script's attack! forwards the command and operands unchanged to
    // sv_module_access::attack. Vanilla charge-shot scripts select two hitbox IDs.
    signature(
        "attack!",
        "Interpolate attacks",
        "agent",
        &[
            ("Command", Exact("*MA_MSC_CMD_ATTACK_SET_LERP")),
            ("Weak attack ID", Unsigned),
            ("Strong attack ID", Unsigned),
        ],
    ),
    signature(
        "attack!",
        "Move attack node",
        "agent",
        &[
            ("Command", Exact("*MA_MSC_CMD_ATTACK_NODE")),
            ("Attack ID", Unsigned),
            ("Bone", Hash),
            ("X", Number),
            ("Y", Number),
            ("Z", Number),
        ],
    ),
    signature(
        "slope!",
        "Slope status",
        "agent",
        &[
            ("Command", Exact("*MA_MSC_CMD_SLOPE_SLOPE")),
            (
                "Status",
                Integer(&["SLOPE_STATUS_PART", "SLOPE_STATUS_TPART"]),
            ),
        ],
    ),
    read_signature(
        "WorkModule::get_float",
        "Read work float",
        "agent.module_accessor",
        &[(
            "Slot",
            Work(
                crate::gimmicks::registry::IdentifierKind::WorkFloat,
                &[
                    "FIGHTER_BRAVE_STATUS_SPECIAL_S_WORK_FLOAT_LIGHTNING_OFFSET_X",
                    "FIGHTER_CLOUD_STATUS_FINAL_WORK_FLOAT_ATTACK_TARGET_LR",
                    "FIGHTER_IKE_STATUS_SPECIAL_N_WORK_FLOAT_CHARGE_COUNT",
                    "FIGHTER_INKLING_INSTANCE_WORK_ID_FLOAT_INK_R",
                    "FIGHTER_JACK_INSTANCE_WORK_ID_FLOAT_RESULT_EFFECT_R",
                    "FIGHTER_KAMUI_STATUS_SPECIAL_N_WORK_FLOAT_HOLD_RATE",
                    "FIGHTER_KIRBY_INSTANCE_WORK_ID_FLOAT_INKLING_SPECIAL_N_INK_R",
                    "FIGHTER_LITTLEMAC_STATUS_WORK_ID_FLOAT_SPECIAL_N_CHARGE_RATE",
                    "FIGHTER_LUCARIO_INSTANCE_WORK_ID_FLOAT_ATTACK_AIR_LW_SPEED",
                    "FIGHTER_LUCARIO_INSTANCE_WORK_ID_FLOAT_CURR_AURAPOWER",
                    "FIGHTER_LUCARIO_POWER_PUNCH_STATUS_WORK_ID_FLOAT_HAKKEI_NEAR_SCALE",
                    "FIGHTER_LUCARIO_SPECIAL_N_STATUS_WORK_ID_FLOAT_CHARGE_RATE",
                    "FIGHTER_LUIGI_STATUS_SPECIAL_S_CHARGE_WORK_FLOAT_CHARGE",
                    "FIGHTER_MIIFIGHTER_INSTANCE_WORK_ID_FLOAT_NENSYO_KICK_CHARGE_RATE",
                    "FIGHTER_MIISWORDSMAN_STATUS_SHIPPU_SLASH_WORK_FLOAT_CHARGE_RATE",
                    "FIGHTER_PACKUN_STATUS_SPECIAL_LW_WORK_FLOAT_CHARGE_RATE",
                    "FIGHTER_PACKUN_STATUS_SPECIAL_N_WORK_FLOAT_SHOOT_START_SPIKEBALL_DISTANCE",
                    "FIGHTER_PIKMIN_STATUS_PULL_OUT_WORK_FLOAT_MOT_RATE",
                    "FIGHTER_PURIN_STATUS_SPECIAL_N_WORK_FLOAT_SPEED",
                    "FIGHTER_REFLET_INSTANCE_WORK_ID_FLOAT_SPECIAL_S_CURRENT_POINT",
                    "FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BEAM_ENERGY_VALUE",
                    "FIGHTER_ROBOT_STATUS_BEAM_WORK_FLOAT_ENERGY_VALUE",
                    "FIGHTER_STATUS_ATTACK_WORK_FLOAT_SMASH_RESTART_FRAME",
                    "WEAPON_BAYONETTA_BAT_INSTANCE_WORK_ID_FLOAT_MOTION_RATE",
                    "WEAPON_EDGE_FLASH_INSTANCE_WORK_ID_FLOAT_ATTACK_RATIO",
                    "WEAPON_GEKKOUGA_SHURIKEN_INSTANCE_WORK_ID_FLOAT_CHARGE_LEVEL_RATIO",
                    "WEAPON_INKLING_BLASTER_INSTANCE_WORK_ID_FLOAT_R",
                    "WEAPON_INKLING_BRUSH_INSTANCE_WORK_ID_FLOAT_R",
                    "WEAPON_INKLING_INKBULLET_INSTANCE_WORK_ID_FLOAT_R",
                    "WEAPON_INKLING_MEGAPHONELASER_INSTANCE_WORK_ID_FLOAT_R",
                    "WEAPON_INKLING_ROLLER_INSTANCE_WORK_ID_FLOAT_R",
                    "WEAPON_INKLING_SLOSHER_INSTANCE_WORK_ID_FLOAT_R",
                    "WEAPON_INKLING_SPLASHBOMB_INSTANCE_WORK_ID_FLOAT_R",
                    "WEAPON_INKLING_SPLASH_INSTANCE_WORK_ID_FLOAT_R",
                    "WEAPON_INKLING_SQUID_INSTANCE_WORK_ID_FLOAT_R",
                    "WEAPON_KAMUI_DRAGONHAND_INSTANCE_WORK_ID_FLOAT_HOLD_RATE",
                    "WEAPON_KAMUI_RYUSENSYA_INSTANCE_WORK_ID_FLOAT_HOLD_RATE",
                    "WEAPON_KOOPAJR_CANNONBALL_INSTANCE_WORK_ID_FLOAT_CHARGE",
                    "WEAPON_LUCARIO_AURABALL_INSTANCE_WORK_ID_FLOAT_AURAPOWER",
                    "WEAPON_LUCARIO_AURABALL_INSTANCE_WORK_ID_FLOAT_CHARGE_RATE",
                    "WEAPON_LUCARIO_AURABALL_INSTANCE_WORK_ID_FLOAT_SE_AURAPOWER_MIDDLE",
                    "WEAPON_LUCAS_PK_FREEZE_INSTANCE_WORK_ID_FLOAT_COUNT",
                    "WEAPON_MEWTWO_SHADOWBALL_INSTANCE_WORK_ID_FLOAT_CHARGE_RATE",
                    "WEAPON_MIIGUNNER_GUNNERCHARGE_INSTANCE_WORK_ID_FLOAT_CHARGE",
                    "WEAPON_MIIGUNNER_STEALTHBOMB_S_INSTANCE_WORK_ID_FLOAT_RATE",
                    "WEAPON_NESS_PK_FLASH_INSTANCE_WORK_ID_FLOAT_COUNT",
                    "WEAPON_PIKMIN_PIKMIN_STATUS_PULL_OUT_START_WORK_FLOAT_MOT_RATE",
                    "WEAPON_RIDLEY_BREATH_INSTANCE_WORK_ID_FLOAT_CHARGE_RATE",
                    "WEAPON_ROSETTA_TICO_STATUS_SPECIAL_N_COMMON_WORK_FLOAT_RATE",
                    "WEAPON_SAMUS_CSHOT_INSTANCE_WORK_ID_FLOAT_CHARGE",
                    "WEAPON_WIIFIT_SUNBULLET_INSTANCE_WORK_ID_FLOAT_CHARGE_LEVEL_RATIO",
                    "WEAPON_ZELDA_DEIN_S_INSTANCE_WORK_ID_FLOAT_RATE",
                    "FIGHTER_STATUS_WORK_ID_FLOAT_RESERVE_HOLD_RATE",
                    "FIGHTER_SZEROSUIT_STATUS_SPECIAL_N_WORK_FLOAT_CHARGE",
                ],
            ),
        )],
    ),
    read_signature(
        "WorkModule::get_int",
        "Read work integer",
        "agent.module_accessor",
        &[(
            "Slot",
            Work(
                crate::gimmicks::registry::IdentifierKind::WorkInt,
                &[
                    "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_COSTUME_KIND",
                    "FIGHTER_IKE_STATUS_SPECIAL_N_WORK_INT_READY_LEVEL",
                    "FIGHTER_INSTANCE_WORK_ID_INT_COLOR",
                    "FIGHTER_INSTANCE_WORK_ID_INT_KIND",
                    "FIGHTER_MASTER_STATUS_SPECIAL_LW_INT_VOICE_TYPE",
                    "FIGHTER_PEACH_STATUS_SPECIAL_LW_WORK_INT_IS_DAIKON_GENERATABLE",
                    "FIGHTER_PIKMIN_STATUS_ATTACK_AIR_WORK_INT_HAVE_PIKMIN",
                    "FIGHTER_REFLET_INSTANCE_WORK_ID_INT_CATCH_EFFECT_HANDLE",
                    "FIGHTER_RIDLEY_INSTANCE_WORK_ID_INT_DISABLE_SPECIAL_LW_FINISH_COUNT",
                    "FIGHTER_ROY_STATUS_SPECIAL_N_WORK_INT_READY_LEVEL",
                ],
            ),
        )],
    ),
    read_signature(
        "WorkModule::is_flag",
        "Read work flag",
        "agent.module_accessor",
        &[(
            "Flag",
            Work(crate::gimmicks::registry::IdentifierKind::WorkFlag, &[]),
        )],
    ),
    read_signature(
        "MotionModule::motion_kind",
        "Read motion kind",
        "agent.module_accessor",
        &[],
    ),
    read_signature(
        "MotionModule::frame",
        "Read motion frame",
        "agent.module_accessor",
        &[],
    ),
    read_signature(
        "MotionModule::rate",
        "Read motion rate",
        "agent.module_accessor",
        &[],
    ),
    read_signature(
        "StatusModule::status_kind",
        "Read status",
        "agent.module_accessor",
        &[],
    ),
    read_signature(
        "KineticModule::get_sum_speed_y",
        "Read vertical speed",
        "agent.module_accessor",
        &[(
            "Reserve",
            Integer(&["KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN"]),
        )],
    ),
    signature(
        "grab!",
        "Clear grabs",
        "agent",
        &[("Command", Exact("*MA_MSC_CMD_GRAB_CLEAR_ALL"))],
    ),
    signature(
        "grab!",
        "Clear grab",
        "agent",
        &[
            ("Command", Exact("*MA_MSC_CMD_GRAB_CLEAR")),
            ("Grab ID", Integer(&[])),
        ],
    ),
    signature(
        "sound!",
        "Stop status sounds",
        "agent",
        &[("Command", Exact("*MA_MSC_CMD_SOUND_STOP_SE_STATUS"))],
    ),
    signature(
        "sound!",
        "Read status sound",
        "agent",
        &[
            ("Command", Exact("*MA_MSC_CMD_SOUND_IS_PLAYING_STATUS")),
            ("Sound", Hash),
        ],
    ),
    signature(
        "slope!",
        "Interpolate slope",
        "agent",
        &[
            ("Command", Exact("*MA_MSC_CMD_SLOPE_SLOPE_INTP")),
            (
                "Status",
                Integer(&["SLOPE_STATUS_PART", "SLOPE_STATUS_TPART"]),
            ),
            ("Frames", Integer(&[])),
        ],
    ),
    signature(
        "slope!",
        "Interpolate slope",
        "agent",
        &[
            ("Command", Exact("*MA_MSC_CMD_SLOPE_SLOPE_INTP")),
            (
                "Status",
                Integer(&["SLOPE_STATUS_PART", "SLOPE_STATUS_TPART"]),
            ),
            ("Frames", Integer(&[])),
            ("Flag", Bool),
        ],
    ),
    signature(
        "macros::FT_SHOOTING_ATTACK_GROUND_CHECK_NEW_arg5",
        "Shooting ground check with feedback",
        "agent",
        &[
            ("Argument 1", Number),
            ("Argument 2", Number),
            ("Argument 3", Number),
            ("Effect", Hash),
            ("Sound", Hash),
        ],
    ),
    signature(
        "macros::EFFECT_OFF_KIND",
        "Stop effect",
        "agent",
        &[("Effect", Hash), ("Fade", Bool), ("Detach", Bool)],
    ),
    signature(
        "VisibilityModule::set_status_default_int64",
        "Default visibility status",
        "agent.module_accessor",
        &[("Group", HashInteger), ("Status", HashInteger)],
    ),
    signature(
        "ItemModule::set_attach_item_visibility",
        "Attached item visibility",
        "agent.module_accessor",
        &[
            ("Visible", Bool),
            (
                "Group",
                Byte(&[
                    "ATTACH_ITEM_GROUP_ALL",
                    "ATTACH_ITEM_GROUP_BACK",
                    "ATTACH_ITEM_GROUP_BADGE",
                    "ATTACH_ITEM_GROUP_BODY",
                    "ATTACH_ITEM_GROUP_HAT_ABNORMAL",
                    "ATTACH_ITEM_GROUP_HAT",
                    "ATTACH_ITEM_GROUP_HIP",
                    "ATTACH_ITEM_GROUP_NONE",
                ]),
            ),
        ],
    ),
    signature(
        "AttackModule::set_attack_camera_quake_forced",
        "Attack camera quake",
        "agent.module_accessor",
        &[
            ("Attack ID", Integer(&[])),
            (
                "Quake",
                Integer(&[
                    "CAMERA_QUAKE_KIND_NONE",
                    "CAMERA_QUAKE_KIND_S",
                    "CAMERA_QUAKE_KIND_M",
                    "CAMERA_QUAKE_KIND_L",
                    "CAMERA_QUAKE_KIND_XL",
                ]),
            ),
            ("Flag", Bool),
        ],
    ),
    signature(
        "GrabModule::set_constraint",
        "Constrain grab",
        "agent.module_accessor",
        &[("Grab ID", Integer(&[])), ("Enabled", Bool)],
    ),
    signature(
        "AttackModule::set_invalid_xlu",
        "Ignore translucency",
        "agent.module_accessor",
        &[("Attack ID", Integer(&[])), ("Enabled", Bool)],
    ),
    signature(
        "AttackModule::set_no_finish_camera_ex",
        "Disable finish camera",
        "agent.module_accessor",
        &[
            ("Attack ID", Integer(&[])),
            ("Disabled", Bool),
            ("Flag 2", Bool),
        ],
    ),
    signature(
        "ModelModule::disable_gold_eye",
        "Disable gold eyes",
        "agent.module_accessor",
        &[],
    ),
    signature(
        "ArticleModule::generate_article",
        "Generate article",
        "agent.module_accessor",
        &[
            ("Article", Article),
            ("Flag", Bool),
            ("Index", Integer(&[])),
        ],
    ),
    signature(
        "EffectModule::remove_screen",
        "Remove screen effect",
        "agent.module_accessor",
        &[("Effect", Hash), ("Index", Integer(&[]))],
    ),
    signature(
        "macros::CANCEL_FILL_SCREEN",
        "Cancel screen fill",
        "agent",
        &[("Index", Integer(&[])), ("Argument 2", Number)],
    ),
];

const fn signature(
    callee: &'static str,
    caption: &'static str,
    receiver: &'static str,
    arguments: &'static [(&'static str, ArgumentKind)],
) -> Signature {
    Signature {
        callee,
        caption,
        receiver,
        arguments,
        stack_wrapper: false,
        bindable: false,
    }
}

const fn read_signature(
    callee: &'static str,
    caption: &'static str,
    receiver: &'static str,
    arguments: &'static [(&'static str, ArgumentKind)],
) -> Signature {
    Signature {
        bindable: true,
        ..signature(callee, caption, receiver, arguments)
    }
}

fn signatures() -> impl Iterator<Item = &'static Signature> {
    SIGNATURES.iter().chain(common::SIGNATURES)
}

// Exact profile operands measured in the dump and confirmed in pinned lua_const.rs.
const CLIFF_PROFILES: &[(&str, &str)] = &[
    ("*", "FIGHTER_CLIFF_HANG_DATA_DEFAULT"),
    ("diddy", "FIGHTER_DIDDY_CLIFF_HANG_DATA_SPECIAL_HI"),
    ("jack", "FIGHTER_JACK_CLIFF_HANG_DATA_AIR_LASSO"),
    ("lucas", "FIGHTER_LUCAS_CLIFF_HANG_DATA_SPECIAL_HI"),
    ("lucas", "FIGHTER_LUCAS_CLIFF_HANG_DATA_START"),
    ("master", "FIGHTER_MASTER_CLIFF_HANG_DATA_AIR_LASSO"),
    ("mewtwo", "FIGHTER_MEWTWO_CLIFF_HANG_DATA_SPECIAL_HI"),
    ("ness", "FIGHTER_NESS_CLIFF_HANG_DATA_SPECIAL_HI"),
    ("ness", "FIGHTER_NESS_CLIFF_HANG_DATA_DEFAULT"),
    ("tantan", "FIGHTER_TANTAN_CLIFF_HANG_DATA_AIR_LASSO"),
];

impl NativeCall {
    pub fn signature(&self) -> Option<&'static Signature> {
        if self.source_line.is_some() {
            return Some(&SOURCE_LINE_SIGNATURE);
        }
        signatures().find(|signature| {
            signature.callee == self.callee
                && signature.arguments.len() == self.arguments.len()
                && self
                    .arguments
                    .iter()
                    .zip(signature.arguments)
                    .all(|(value, (_, kind))| !matches!(kind, Exact(_)) || kind.accepts(value))
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        if let Some(line) = &self.source_line {
            return if line.sdk_arity().is_some()
                && self.callee.is_empty()
                && self.arguments.is_empty()
                && self.receiver.is_none()
                && self.binding.is_none()
            {
                Ok(())
            } else {
                Err("Invalid opaque source line".into())
            };
        }
        let signature = self.signature().ok_or("Unknown native call")?;
        if let Some(binding) = &self.binding {
            if !signature.bindable
                || syn::parse_str::<syn::Ident>(binding).is_err()
                || matches!(binding.trim_start_matches("r#"), "agent" | "boma")
            {
                return Err("Invalid native binding".into());
            }
        }
        if let Some(receiver) = &self.receiver {
            if signature.receiver != "agent.module_accessor" || receiver != "boma" {
                return Err("Invalid native receiver".into());
            }
        }
        if self.arguments.len() != signature.arguments.len() {
            return Err(format!(
                "{} requires {} arguments",
                self.callee,
                signature.arguments.len()
            ));
        }
        if matches!(
            self.callee.as_str(),
            "WorkModule::on_flag" | "WorkModule::off_flag"
        ) && self.arguments.first().is_some_and(|value| {
            super::native_work::read_only_condition_flags()
                .iter()
                .any(|symbol| value == &format!("*{symbol}"))
        }) {
            return Err("Read-only native flag".into());
        }
        for (value, (label, kind)) in self.arguments.iter().zip(signature.arguments) {
            if !kind.accepts(value) {
                return Err(format!("Invalid {label} for {}", self.callee));
            }
        }
        Ok(())
    }

    pub fn validate_for_fighter(&self, fighter: Option<&str>) -> Result<(), String> {
        self.validate()?;
        if self.callee == "GroundModule::select_cliff_hangdata" {
            let fighter = fighter.ok_or("Cliff profile selection requires a fighter")?;
            if crate::gimmicks::registry::fighter_kind_symbol(fighter).is_none()
                || !CLIFF_PROFILES.iter().any(|(owner, symbol)| {
                    (*owner == "*" || *owner == fighter)
                        && self.arguments[0] == format!("*{symbol} as u32")
                })
            {
                return Err("Cliff profile belongs to another fighter".into());
            }
        }
        if self.callee == "ArticleModule::set_visibility" {
            let fighter = fighter.ok_or("Article visibility requires a fighter")?;
            let expr = syn::parse_str::<syn::Expr>(&self.arguments[0])
                .map_err(|error| error.to_string())?;
            let symbol =
                constant_symbol(&expr).ok_or("Article visibility requires a registered article")?;
            crate::gimmicks::registry::resolve(
                fighter,
                &symbol,
                crate::gimmicks::registry::IdentifierKind::Article,
            )
            .map_err(|error| error.to_string())?;
        }
        if self.callee == "StatusModule::change_status_request_from_script" {
            let fighter = fighter.ok_or("Changing status requires a fighter")?;
            let expr = syn::parse_str::<syn::Expr>(&self.arguments[0])
                .map_err(|error| error.to_string())?;
            let symbol = constant_symbol(&expr)
                .ok_or("Changing status requires a registered fighter status")?;
            crate::gimmicks::registry::resolve(
                fighter,
                &symbol,
                crate::gimmicks::registry::IdentifierKind::FighterStatus,
            )
            .map_err(|error| error.to_string())?;
        }
        if matches!(
            self.callee.as_str(),
            "WorkModule::on_flag" | "WorkModule::off_flag"
        ) {
            let fighter = fighter.ok_or("Native flag editing requires a fighter")?;
            super::native_work::validate_flag(fighter, &self.arguments[0])?;
        }
        Ok(())
    }

    pub fn source(&self) -> String {
        if let Some(line) = &self.source_line {
            return line.source.clone();
        }
        let receiver = self
            .receiver
            .as_deref()
            .unwrap_or_else(|| self.signature().map_or("", |signature| signature.receiver));
        let mut arguments = vec![receiver.to_owned()];
        arguments.extend(self.arguments.iter().cloned());
        let binding = self
            .binding
            .as_ref()
            .map(|binding| format!("let {binding} = "))
            .unwrap_or_default();
        format!("{binding}{}({});", self.callee, arguments.join(", "))
    }
}

impl ArgumentKind {
    fn normalize(self, source: &str) -> Option<String> {
        if self.accepts(source) {
            return Some(source.into());
        }
        match self {
            ScreenLayer => match source {
                "EffectScreenLayer:*GROUND" | "*EffectScreenLayer::GROUND" => Some("0".into()),
                "EffectScreenLayer:*CHAR" | "*EffectScreenLayer::CHAR" => Some("1".into()),
                "EffectScreenLayer:*ALL" | "*EffectScreenLayer::ALL" => Some("2".into()),
                _ => None,
            },
            Float(true) => normalize_float(source),
            HashInteger if Hash.accepts(source) => Some(format!("{source}.hash as i64")),
            Byte(choices)
                if syn::parse_str::<syn::Expr>(source)
                    .ok()
                    .and_then(|expr| constant_symbol(&expr))
                    .is_some_and(|symbol| choices.contains(&symbol.as_str())) =>
            {
                Some(format!("{source} as u8"))
            }
            _ => None,
        }
    }
    fn choices(self) -> Vec<&'static str> {
        match self {
            Integer(choices) | Byte(choices) => choices.to_vec(),
            Work(kind, extra) => {
                let mut choices = crate::gimmicks::registry::ENTRIES
                    .iter()
                    .filter(|entry| entry.kind == kind)
                    .map(|entry| entry.symbol)
                    .chain(extra.iter().copied())
                    .collect::<Vec<_>>();
                if kind == crate::gimmicks::registry::IdentifierKind::WorkFlag {
                    choices.extend(super::native_work::common_flags());
                    choices.extend(super::native_work::read_only_condition_flags());
                    choices.extend(
                        super::native_status_flags::entries()
                            .iter()
                            .map(|entry| entry.symbol),
                    );
                }
                choices.sort_unstable();
                choices.dedup();
                choices
            }
            _ => Vec::new(),
        }
    }
    pub fn accepts(self, source: &str) -> bool {
        if source.len() > 512 {
            return false;
        }
        let Ok(expr) = syn::parse_str::<syn::Expr>(source) else {
            return false;
        };
        match self {
            Exact(value) => source == value,
            Work(kind, extra) => constant_symbol(&expr).is_some_and(|symbol| {
                extra.contains(&symbol.as_str())
                    || crate::gimmicks::registry::ENTRIES
                        .iter()
                        .any(|entry| entry.symbol == symbol && entry.kind == kind)
                    || (kind == crate::gimmicks::registry::IdentifierKind::WorkFlag
                        && (super::native_work::common_flags().contains(&symbol.as_str())
                            || super::native_work::read_only_condition_flags()
                                .contains(&symbol.as_str())
                            || super::native_status_flags::contains_symbol(&symbol)))
            }),
            Number => numeric(&expr, false),
            Float(_) => numeric(&expr, false) && float_literal(&expr),
            HashInteger => source
                .strip_suffix(".hash as i64")
                .is_some_and(|hash| Hash.accepts(hash)),
            Byte(choices) => {
                matches!(&expr, syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(value), attrs, .. }) if attrs.is_empty() && matches!(value.suffix(), "" | "u8") && value.base10_parse::<u8>().is_ok())
                    || source
                        .strip_suffix(" as u8")
                        .and_then(|value| syn::parse_str::<syn::Expr>(value).ok())
                        .and_then(|expr| constant_symbol(&expr))
                        .is_some_and(|symbol| choices.contains(&symbol.as_str()))
            }
            Unsigned => {
                matches!(expr, syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(ref value), ref attrs, .. }) if attrs.is_empty() && matches!(value.suffix(), "" | "u64") && value.base10_parse::<u64>().is_ok())
            }
            ScreenLayer => matches!(source, "0" | "1" | "2"),
            SimdVector(count) => {
                let syn::Expr::Call(call) = &expr else {
                    return false;
                };
                let syn::Expr::Path(path) = call.func.as_ref() else {
                    return false;
                };
                let expected = format!("smash::phx::simd::Vector{count}::new");
                path.qself.is_none()
                    && path
                        .path
                        .segments
                        .iter()
                        .map(|s| s.ident.to_string())
                        .collect::<Vec<_>>()
                        .join("::")
                        == expected
                    && call.args.len() == count
                    && call
                        .args
                        .iter()
                        .all(|arg| numeric(arg, false) && float_literal(arg))
            }
            Vector3 => vector3(&expr),
            CliffProfile => CLIFF_PROFILES
                .iter()
                .any(|(_, symbol)| source == format!("*{symbol} as u32")),
            Article => constant_symbol(&expr).is_some_and(|symbol| {
                crate::gimmicks::registry::ENTRIES.iter().any(|entry| {
                    entry.symbol == symbol
                        && entry.kind == crate::gimmicks::registry::IdentifierKind::Article
                })
            }),
            Integer(choices) => {
                numeric(&expr, true)
                    || choices.iter().any(|symbol| {
                        source == format!("*{symbol}") || source == format!("*lua_const::{symbol}")
                    })
            }
            Bool => matches!(
                expr,
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Bool(_), attrs, ..
                }) if attrs.is_empty()
            ),
            Hash => {
                match expr {
                    syn::Expr::Call(call) if call.attrs.is_empty() && call.args.len() == 1 => {
                        let syn::Expr::Path(path) = *call.func else {
                            return false;
                        };
                        if !path.attrs.is_empty()
                            || path.qself.is_some()
                            || path.path.leading_colon.is_some()
                            || path.path.segments.iter().any(|segment| {
                                !matches!(segment.arguments, syn::PathArguments::None)
                            })
                        {
                            return false;
                        }
                        let path = path
                            .path
                            .segments
                            .iter()
                            .map(|segment| segment.ident.to_string())
                            .collect::<Vec<_>>()
                            .join("::");
                        match (path.as_str(), call.args.first()) {
                            (
                                "Hash40::new",
                                Some(syn::Expr::Lit(syn::ExprLit {
                                    lit: syn::Lit::Str(_),
                                    attrs,
                                    ..
                                })),
                            ) => attrs.is_empty(),
                            (
                                "Hash40::new_raw",
                                Some(syn::Expr::Lit(syn::ExprLit {
                                    lit: syn::Lit::Int(value),
                                    attrs,
                                    ..
                                })),
                            ) => {
                                attrs.is_empty()
                                    && matches!(value.suffix(), "" | "u64")
                                    && value.base10_parse::<u64>().is_ok()
                            }
                            _ => false,
                        }
                    }
                    _ => false,
                }
            }
        }
    }
}

fn constant_symbol(expr: &syn::Expr) -> Option<String> {
    let syn::Expr::Unary(value) = expr else {
        return None;
    };
    if !value.attrs.is_empty() || !matches!(value.op, syn::UnOp::Deref(_)) {
        return None;
    }
    let syn::Expr::Path(path) = value.expr.as_ref() else {
        return None;
    };
    if !path.attrs.is_empty()
        || path.qself.is_some()
        || path.path.leading_colon.is_some()
        || path
            .path
            .segments
            .iter()
            .any(|segment| !matches!(segment.arguments, syn::PathArguments::None))
    {
        return None;
    }
    let parts = path
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [symbol] => Some(symbol.clone()),
        [module, symbol] if module == "lua_const" => Some(symbol.clone()),
        _ => None,
    }
}

fn float_literal(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Float(value),
            ..
        }) => matches!(value.suffix(), "" | "f32"),
        syn::Expr::Unary(value) if matches!(value.op, syn::UnOp::Neg(_)) => {
            float_literal(&value.expr)
        }
        _ => false,
    }
}

fn normalize_float(source: &str) -> Option<String> {
    let expr = syn::parse_str::<syn::Expr>(source).ok()?;
    if Float(false).accepts(source) {
        return Some(source.into());
    }
    if numeric(&expr, true) && !source.ends_with("i32") {
        return Some(format!("{source}.0"));
    }
    None
}

fn vector3(expr: &syn::Expr) -> bool {
    let syn::Expr::Struct(value) = expr else {
        return false;
    };
    value.attrs.is_empty()
        && value.path.is_ident("Vector3f")
        && value.qself.is_none()
        && value.rest.is_none()
        && value.fields.len() == 3
        && ["x", "y", "z"].iter().all(|name| {
            value.fields.iter().any(|field| {
                field.attrs.is_empty()
                    && field.colon_token.is_some()
                    && matches!(&field.member, syn::Member::Named(ident) if ident == name)
                    && numeric(&field.expr, false)
                    && float_literal(&field.expr)
            })
        })
}

fn numeric(expr: &syn::Expr, integer: bool) -> bool {
    match expr {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(value),
            attrs,
            ..
        }) if attrs.is_empty() => {
            matches!(value.suffix(), "" | "i32") && value.base10_parse::<i32>().is_ok()
        }
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Float(value),
            attrs,
            ..
        }) if !integer && attrs.is_empty() => {
            matches!(value.suffix(), "" | "f32" | "f64")
                && value.base10_parse::<f32>().is_ok_and(f32::is_finite)
        }
        syn::Expr::Unary(value)
            if value.attrs.is_empty() && matches!(value.op, syn::UnOp::Neg(_)) =>
        {
            numeric(&value.expr, integer)
        }
        _ => false,
    }
}

// Dumped MSC calls omit the implicit Lua stack receiver. Resolve only reviewed
// stack entry points; module calls with missing SDK arguments remain preserved.
fn implicit_msc_callee(callee: &str) -> Option<&'static str> {
    match callee {
        "physics" => Some("physics!"),
        "shield" => Some("shield!"),
        "THROW_ITEM_arg3" => Some("macros::THROW_ITEM_arg3"),
        "REQ_FINAL_START_CAMERA_arg3" => Some("macros::REQ_FINAL_START_CAMERA_arg3"),
        "ATK_SET_SHIELD_SETOFF_MUL2" => Some("macros::ATK_SET_SHIELD_SETOFF_MUL"),
        _ => None,
    }
}

pub fn contains_supported_call(source: &str) -> bool {
    SDK_ARITIES
        .iter()
        .any(|(callee, _)| source.contains(callee))
        || SIGNATURES
            .iter()
            .any(|signature| source.contains(signature.callee))
        || source.contains("methodlua2cpp::L2CFighterAnimcmd")
        || source.contains("smash::lua2cpp::L2CFighterAnimcmd")
        || source.contains("physics(")
        || source.contains("shield(")
        || [
            "THROW_ITEM_arg3(",
            "REQ_FINAL_START_CAMERA_arg3(",
            "ATK_SET_SHIELD_SETOFF_MUL2(",
        ]
        .iter()
        .any(|callee| source.contains(callee))
}

pub fn parse(source: &str) -> Option<NativeCall> {
    let source = source.trim();
    if let Some(method) = source
        .strip_prefix("methodlua2cpp::")
        .and_then(|s| s.strip_suffix("()();"))
    {
        let (class, method) = method.split_once("::")?;
        let callee = format!("smash::lua2cpp::{class}_{method}");
        let signature = common::SIGNATURES.iter().find(|s| s.callee == callee)?;
        return Some(NativeCall {
            callee: signature.callee.into(),
            arguments: Vec::new(),
            receiver: None,
            binding: None,
            source_line: None,
        });
    }
    let (binding, source) = if let Some(local) = source.strip_prefix("let ") {
        let (binding, value) = local.split_once(" = ")?;
        (Some(binding.to_owned()), value)
    } else {
        (None, source)
    };
    let (callee, values) = source.strip_suffix(");")?.split_once('(')?;
    let mut values = crate::acmd::tokenize_args_balanced(values);
    // The dump's named-operand spelling is one closed alias of the native stack macro.
    // Keep each key tied to its slot and reject swapped, missing or extra operands.
    if callee == "ATK_SET_SHIELD_SETOFF_MUL2" {
        let [id, multiplier] = values.as_slice() else {
            return None;
        };
        values = vec![
            id.strip_prefix("ID=")?.to_owned(),
            multiplier.strip_prefix("ShieldstunMul=")?.to_owned(),
        ];
    }
    let implicit_receiver = implicit_msc_callee(callee);
    let callee = implicit_receiver.unwrap_or(callee);
    let argument_start = usize::from(implicit_receiver.is_none());
    for signature in signatures().filter(|signature| {
        signature.callee == callee && values.len() == signature.arguments.len() + argument_start
    }) {
        let receiver = if implicit_receiver.is_some() || values[0] == signature.receiver {
            None
        } else if signature.receiver == "agent.module_accessor" && values[0] == "boma" {
            Some(values[0].clone())
        } else {
            continue;
        };
        let arguments = values
            .iter()
            .skip(argument_start)
            .zip(signature.arguments)
            .map(|(value, (_, kind))| kind.normalize(value))
            .collect::<Option<Vec<_>>>();
        let Some(arguments) = arguments else {
            continue;
        };
        let call = NativeCall {
            callee: signature.callee.into(),
            arguments,
            receiver,
            binding: binding.clone(),
            source_line: None,
        };
        if call.validate().is_ok() {
            return Some(call);
        }
    }
    None
}

/// Recognize an incomplete decompiler statement without claiming native call semantics.
pub fn parse_source_line(source: &str) -> Option<NativeCall> {
    let line = SourceLine {
        source: source.trim().into(),
    };
    line.sdk_arity()?;
    Some(NativeCall {
        callee: String::new(),
        arguments: Vec::new(),
        receiver: None,
        binding: None,
        source_line: Some(line),
    })
}

pub fn support_source(source: &str) -> String {
    if !source.contains("singletons::FighterCutInManager()") {
        return String::new();
    }
    r#"#[allow(non_snake_case)]
mod singletons {
    extern "C" {
        #[link_name = "_ZN3lib9SingletonIN3app19FighterCutInManagerEE9instance_E"]
        static FIGHTER_CUT_IN_MANAGER: *mut smash::app::FighterCutInManager;
    }
    pub unsafe fn FighterCutInManager() -> *mut smash::app::FighterCutInManager {
        FIGHTER_CUT_IN_MANAGER
    }
}
"#
    .into()
}

pub fn compatibility_wrappers(used: &std::collections::BTreeSet<String>) -> String {
    let mut output = String::new();
    for signature in SIGNATURES
        .iter()
        .filter(|signature| signature.stack_wrapper)
    {
        let name = signature.callee.strip_prefix("macros::").unwrap();
        if !used.contains(name) {
            continue;
        }
        let mut generics = Vec::new();
        let mut parameters = vec!["agent: &mut L2CAgentBase".into()];
        let mut arguments = Vec::new();
        for (index, (_, kind)) in signature.arguments.iter().enumerate() {
            let typ = match kind {
                Bool => "bool".into(),
                Hash => "Hash40".into(),
                _ => {
                    generics.push(format!("A{index}: Into<smash::lib::L2CValue>"));
                    format!("A{index}")
                }
            };
            parameters.push(format!("a{index}: {typ}"));
            arguments.push(format!("a{index}"));
        }
        output.push_str(&format!(
            "    pub unsafe fn {name}<{}>({}) {{\n        agent.clear_lua_stack();\n        smash_script::lua_args!(agent, {});\n        smash::app::sv_animcmd::{name}(agent.lua_state_agent);\n        agent.clear_lua_stack();\n    }}\n",
            generics.join(", "), parameters.join(", "), arguments.join(", ")
        ));
    }
    output
}

pub fn inspector(ui: &mut egui::Ui, call: &mut NativeCall) {
    inspector_for_fighter(ui, call, "");
}

pub fn inspector_for_fighter(ui: &mut egui::Ui, call: &mut NativeCall, fighter: &str) {
    if let Some(line) = &call.source_line {
        ui.label("Opaque source line");
        ui.monospace(&line.source);
        if let Some((expected, actual)) = line.sdk_arity() {
            ui.weak(format!("Decompiler supplied {actual} of {expected} SDK arguments. Missing values are unresolved."));
        }
        return;
    }
    let Some(signature) = call.signature() else {
        return;
    };
    if let Some(binding) = &call.binding {
        ui.label(format!("let {binding}"));
    }
    for (index, (argument, (label, kind))) in call
        .arguments
        .iter_mut()
        .zip(signature.arguments)
        .enumerate()
    {
        if signature.callee == "StatusModule::change_status_request_from_script"
            && matches!(kind, Exact(_))
        {
            continue;
        }
        ui.push_id(index, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(*label);
                match kind {
                    Exact(_) => {
                        ui.label(argument.as_str());
                    }
                    Bool => {
                        let mut enabled = argument == "true";
                        if ui.checkbox(&mut enabled, "").changed() {
                            *argument = enabled.to_string();
                        }
                    }
                    Float(_) => {
                        let mut value = argument
                            .trim_end_matches("f32")
                            .parse::<f32>()
                            .unwrap_or_default();
                        if ui
                            .add(egui::DragValue::new(&mut value).speed(0.05))
                            .changed()
                        {
                            *argument = format!("{value:?}");
                        }
                    }
                    CliffProfile | Article => {
                        let choices: Vec<String> = if matches!(kind, CliffProfile) {
                            CLIFF_PROFILES
                                .iter()
                                .filter(|(owner, _)| *owner == "*" || *owner == fighter)
                                .map(|(_, symbol)| format!("*{symbol} as u32"))
                                .collect()
                        } else {
                            crate::gimmicks::registry::entries_for(
                                fighter,
                                crate::gimmicks::registry::IdentifierKind::Article,
                            )
                            .map(|entry| format!("*{}", entry.symbol))
                            .collect()
                        };
                        egui::ComboBox::from_id_salt("native_choice")
                            .selected_text(argument.as_str())
                            .show_ui(ui, |ui| {
                                for value in choices {
                                    ui.selectable_value(argument, value.clone(), value);
                                }
                            });
                    }
                    Work(crate::gimmicks::registry::IdentifierKind::FighterStatus, _) => {
                        let mut symbol = syn::parse_str::<syn::Expr>(argument)
                            .ok()
                            .and_then(|expr| constant_symbol(&expr))
                            .unwrap_or_else(|| argument.clone());
                        let before = symbol.clone();
                        crate::gimmicks::registry::fighter_status_picker(ui, fighter, &mut symbol);
                        if symbol != before {
                            *argument = format!("*{symbol}");
                        }
                    }
                    Integer(_) | Byte(_) | Work(_, _) => {
                        let choices = kind.choices();
                        egui::ComboBox::from_id_salt("constant")
                            .selected_text(argument.as_str())
                            .show_ui(ui, |ui| {
                                for symbol in choices {
                                    let value = format!("*{symbol}");
                                    ui.selectable_value(argument, value.clone(), value);
                                }
                            });
                        ui.text_edit_singleline(argument);
                    }
                    _ => {
                        ui.text_edit_singleline(argument);
                    }
                }
                if let Some(normalized) = kind.normalize(argument) {
                    *argument = normalized;
                }
                if !kind.accepts(argument) {
                    ui.colored_label(egui::Color32::RED, "Invalid value");
                }
            });
        });
    }
}

#[cfg(test)]
pub const CORPUS_LINES: &[&str] = &[
    "macros::THROW_ITEM_OFFSET(agent, 0, 0.1, *ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_ANGLE, *ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_SPEED, *ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_POWER);",
    "macros::SHOOT_ITEM_BULLET_CHARGE(agent, *FIGHTER_STATUS_ITEM_SHOOT_WORK_INT_SSCOPE_CHARGE);",
    "macros::FT_MOTION_INTP_WAIT_ITEM(agent, true, 0.02);",
    "macros::RUMBLE_FINAL_TARGET(agent, Hash40::new(\"rbkind_nohitl\"), 6);",
    "macros::SHOOT_ITEM_BULLET(agent);",
    "macros::FT_START_CUTIN(agent);",
    "macros::LAST_EFFECT_SET_COLOR(agent, 0.35, 0.7, 1.2);",
    "macros::LAST_PARTICLE_SET_COLOR(agent, 0.1, 0.4, 1);",
    "macros::LAST_EFFECT_SET_RATE(agent, 1.6);",
    "macros::FT_SHOOTING_ATTACK_GROUND_CHECK_NEW(agent, 0, 4, 4);",
    "macros::ATK_SET_SHIELD_SETOFF_MUL_arg3(agent, 0, 1, 15.0);",
    "lua_bind::FighterCutInManager::set_throw_finish_zoom_rate(singletons::FighterCutInManager(), 1.3);",
    "lua_bind::FighterCutInManager::set_throw_finish_offset(singletons::FighterCutInManager(), Vector3f{x: 9.5, y: 0.5, z: 0.0});",
    "FighterSpecializer_Demon::set_devil(agent.module_accessor, true, 10);",
    "ArticleModule::change_motion(agent.module_accessor, *FIGHTER_BUDDY_GENERATE_ARTICLE_PARTNER, Hash40::new(\"attack_s3_s\"), false, -1.0);",
    "PostureModule::scale(agent.module_accessor);",

    "let motion = MotionModule::motion_kind(agent.module_accessor);",
    "let frame = MotionModule::frame(agent.module_accessor);",
    "let rate = MotionModule::rate(agent.module_accessor);",
    "let status = StatusModule::status_kind(agent.module_accessor);",
    "let restart_frame = WorkModule::get_float(agent.module_accessor, *FIGHTER_STATUS_ATTACK_WORK_FLOAT_SMASH_RESTART_FRAME);",
    "let get_speed_y = KineticModule::get_sum_speed_y(agent.module_accessor, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);",
    "notify_event_msc_cmd!(agent, Hash40::new_raw(0x2127e37c07), *GROUND_CLIFF_CHECK_KIND_ALWAYS_BOTH_SIDES);",
    "notify_event_msc_cmd!(agent, Hash40::new_raw(0x26769bd1de), -30, 30, 5);",
    "notify_event_msc_cmd!(agent, Hash40::new_raw(0x23c33f3bdc));",
    "notify_event_msc_cmd!(agent, Hash40::new_raw(0x1f20a9d549), false);",
    "notify_event_msc_cmd!(agent, Hash40::new_raw(0x1f5b14bb65), *FIGHTER_ROCKMAN_ARM_LEFT, *FIGHTER_ROCKMAN_ARMFORM_ROCKBUSTER, 0, 120);",
    "notify_event_msc_cmd!(agent, Hash40::new_raw(0x2d51fcdb09), *FIGHTER_BAYONETTA_SHOOTING_SLOT_R_ARM, true, true, true, 10, 0, 3, 0, false);",
    "attack!(agent, *MA_MSC_CMD_ATTACK_NODE, 3, Hash40::new(\"top\"), 0, 9, -5.2);",
    "slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE, *SLOPE_STATUS_TPART);",
    "grab!(agent, *MA_MSC_CMD_GRAB_CLEAR_ALL);",
    "grab!(agent, *MA_MSC_CMD_GRAB_CLEAR, 1);",
    "sound!(agent, *MA_MSC_CMD_SOUND_STOP_SE_STATUS);",
    "sound!(agent, *MA_MSC_CMD_SOUND_IS_PLAYING_STATUS, Hash40::new(\"se_pitb_special_n01\"));",
    "slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE_INTP, *SLOPE_STATUS_TPART, 5);",
    "macros::FT_SHOOTING_ATTACK_GROUND_CHECK_NEW_arg5(agent, 2, 4, 4, Hash40::new(\"jack_gun_hit2\"), Hash40::new(\"se_jack_special_n02\"));",
    "macros::EFFECT_OFF_KIND(agent, Hash40::new(\"metaknight_sword\"), false, false);",
    "VisibilityModule::set_status_default_int64(agent.module_accessor, Hash40::new(\"mantle\"), Hash40::new(\"mantle_wing\"));",
    "ItemModule::set_attach_item_visibility(agent.module_accessor, false, *ATTACH_ITEM_GROUP_BACK);",
    "AttackModule::set_attack_camera_quake_forced(agent.module_accessor, 7, *CAMERA_QUAKE_KIND_M, false);",
    "GrabModule::set_constraint(agent.module_accessor, 0, true);",
    "ArticleModule::generate_article(agent.module_accessor, *FIGHTER_BRAVE_GENERATE_ARTICLE_BLUE, true, -1);",
    "EffectModule::remove_screen(agent.module_accessor, Hash40::new(\"bg_bayonetta_final\"), -1);",
    "macros::CANCEL_FILL_SCREEN(agent, 0, 50);",
    "camera!(agent, *MA_MSC_CMD_CAMERA_CAM_RECT, 60, -56, 18, -8);",
    "search!(agent, *MA_MSC_CMD_SEARCH_SET_OPPONENT, 0, 0, *COLLISION_TARGET_PROPERTY, *COLLISION_PROPERTY_MASK_REFLECT);",
    "physics!(agent, *MA_MSC_CMD_PHYSICS_START_CHARGE, 0.7, 0.9, -1, 0.5, 0.5, -1, Hash40::new(\"invalid\"));",
    "macros::FLASH_SET_DIRECTION(agent, -1, 0, 0);",
    "macros::FT_REMOVE_FINAL_AURA(agent, false);",
    "macros::THROW_ITEM_arg3(agent, *ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_ANGLE, *ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_SPEED, *ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_POWER);",
    "macros::REQ_FINAL_START_CAMERA_arg3(agent, Hash40::new(\"d04finalstart.nuanmb\"), false, false);",
    "macros::COL_PRI(agent, 200);",
    "macros::QUAKE(agent, *CAMERA_QUAKE_KIND_S);",
    "macros::EFFECT_FOLLOW_RND(agent, Hash40::new(\"roy_attack_fire\"), Hash40::new(\"sword1\"), 0, 0, 7, 0, 0, 0, 0.6, 1, 1, 8, 0, 0, 0, true);",
    "shield!(agent, *MA_MSC_CMD_SHIELD_OFF, *COLLISION_KIND_REFLECTOR, *FIGHTER_PALUTENA_REFLECTOR_KIND_REFLECTOR, *FIGHTER_REFLECTOR_GROUP_EXTEND);",
    "AttackModule::set_attack_level(agent.module_accessor, 0, *FIGHTER_RYU_SAVING_LV_2 as u8);",
    "AttackModule::set_force_reaction(agent.module_accessor, 0, true, true);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_BAYONETTA_BAT_INSTANCE_WORK_ID_FLOAT_MOTION_RATE);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_EDGE_FLASH_INSTANCE_WORK_ID_FLOAT_ATTACK_RATIO);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_GEKKOUGA_SHURIKEN_INSTANCE_WORK_ID_FLOAT_CHARGE_LEVEL_RATIO);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_INKLING_BLASTER_INSTANCE_WORK_ID_FLOAT_R);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_INKLING_BRUSH_INSTANCE_WORK_ID_FLOAT_R);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_INKLING_INKBULLET_INSTANCE_WORK_ID_FLOAT_R);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_INKLING_MEGAPHONELASER_INSTANCE_WORK_ID_FLOAT_R);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_INKLING_ROLLER_INSTANCE_WORK_ID_FLOAT_R);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_INKLING_SLOSHER_INSTANCE_WORK_ID_FLOAT_R);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_INKLING_SPLASHBOMB_INSTANCE_WORK_ID_FLOAT_R);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_INKLING_SPLASH_INSTANCE_WORK_ID_FLOAT_R);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_INKLING_SQUID_INSTANCE_WORK_ID_FLOAT_R);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_KAMUI_DRAGONHAND_INSTANCE_WORK_ID_FLOAT_HOLD_RATE);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_KAMUI_RYUSENSYA_INSTANCE_WORK_ID_FLOAT_HOLD_RATE);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_KOOPAJR_CANNONBALL_INSTANCE_WORK_ID_FLOAT_CHARGE);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_LUCARIO_AURABALL_INSTANCE_WORK_ID_FLOAT_AURAPOWER);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_LUCARIO_AURABALL_INSTANCE_WORK_ID_FLOAT_CHARGE_RATE);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_LUCARIO_AURABALL_INSTANCE_WORK_ID_FLOAT_SE_AURAPOWER_MIDDLE);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_LUCAS_PK_FREEZE_INSTANCE_WORK_ID_FLOAT_COUNT);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_MEWTWO_SHADOWBALL_INSTANCE_WORK_ID_FLOAT_CHARGE_RATE);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_MIIGUNNER_GUNNERCHARGE_INSTANCE_WORK_ID_FLOAT_CHARGE);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_MIIGUNNER_STEALTHBOMB_S_INSTANCE_WORK_ID_FLOAT_RATE);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_NESS_PK_FLASH_INSTANCE_WORK_ID_FLOAT_COUNT);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_PIKMIN_PIKMIN_STATUS_PULL_OUT_START_WORK_FLOAT_MOT_RATE);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_RIDLEY_BREATH_INSTANCE_WORK_ID_FLOAT_CHARGE_RATE);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_ROSETTA_TICO_STATUS_SPECIAL_N_COMMON_WORK_FLOAT_RATE);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_SAMUS_CSHOT_INSTANCE_WORK_ID_FLOAT_CHARGE);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_WIIFIT_SUNBULLET_INSTANCE_WORK_ID_FLOAT_CHARGE_LEVEL_RATIO);",
    "WorkModule::get_float(agent.module_accessor, *WEAPON_ZELDA_DEIN_S_INSTANCE_WORK_ID_FLOAT_RATE);",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{AcmdStmt, ExcuteStmt};
    use crate::move_nodes::{
        model::{MoveGraph, NodeKind, Track},
        timeline,
    };

    #[test]
    fn corpus_charge_shot_attack_interpolation_is_editable() {
        // SSBU-Dumped-Scripts/smashline/lua2cpp_samus/samus_cshot/Shoot.txt.
        let line = "attack!(agent, *MA_MSC_CMD_ATTACK_SET_LERP, 0, 1);";
        let call = parse(line).expect("charge-shot interpolation must be typed");
        assert_eq!(call.arguments, ["*MA_MSC_CMD_ATTACK_SET_LERP", "0", "1"]);
        assert!(call.source_line.is_none());
        assert_eq!(call.source(), line);
        assert_eq!(parse(&call.source()), Some(call));
        for invalid in [
            "attack!(agent, *MA_MSC_CMD_ATTACK_SET_LERP, 0);",
            "attack!(agent, *MA_MSC_CMD_ATTACK_SET_LERP, -1, 1);",
            "attack!(agent, *MA_MSC_CMD_ATTACK_SET_LERP, 0, injected());",
            "attack!(other, *MA_MSC_CMD_ATTACK_SET_LERP, 0, 1);",
        ] {
            assert!(parse(invalid).is_none(), "{invalid}");
        }
        let source = format!("unsafe extern \"C\" fn game_shoot(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {line}\n    }}\n}}");
        let script = crate::acmd::parse_acmd_script(&source);
        let track = timeline::import_for_fighter("samus", Track::Game, &script);
        let graph = MoveGraph {
            tracks: vec![track],
            ..Default::default()
        };
        let compiled = timeline::compile("samus", "shoot", &graph).unwrap();
        let emitted = crate::acmd::preview_game_fn(&compiled[0].1, "shoot");
        assert!(emitted.contains(line), "{emitted}");
        assert!(syn::parse_file(&emitted).is_ok());
    }

    #[test]
    fn corpus_screen_and_indexed_shield_calls_round_trip() {
        // smashline: murabito/FinalMoneyL, buddy/Attack100, roy/AttackDash,
        // and shulk/SpecialLwF in SSBU-Dumped-Scripts.
        for line in [
            "macros::FILL_SCREEN_MODEL_COLOR(agent, 1, 10, 0.3, 0.3, 0.3, 0, 0, 0, 1, 1.3, EffectScreenLayer:*GROUND, *EFFECT_SCREEN_PRIO_FINAL);",
            "macros::CANCEL_FILL_SCREEN(agent, 0, 50);",
            "macros::ATK_SET_SHIELD_SETOFF_MUL_arg4(agent, 0, 1, 2, 1.25);",
            "macros::ATK_SET_SHIELD_SETOFF_MUL_arg5(agent, 0, 1, 2, 3, 6);",
            "EffectModule::remove_post_effect_line(agent.module_accessor, 2, true);",
        ] {
            let call = parse(line).unwrap_or_else(|| panic!("{line}"));
            assert!(call.source_line.is_none());
            assert_eq!(parse(&call.source()), Some(call.clone()));
            assert!(call.validate_for_fighter(Some("mario")).is_ok());
            let source = format!("unsafe extern \"C\" fn game_test(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {line}\n    }}\n}}");
            let script = crate::acmd::parse_acmd_script(&source);
            let track = timeline::import_for_fighter("mario", Track::Game, &script);
            let graph = MoveGraph { tracks: vec![track], ..Default::default() };
            let compiled = timeline::compile("mario", "test", &graph).unwrap();
            let emitted = crate::acmd::preview_game_fn(&compiled[0].1, "test");
            assert!(emitted.contains(&call.source()), "{emitted}");
            assert!(syn::parse_file(&emitted).is_ok());
        }
        let call = parse("macros::ATK_SET_SHIELD_SETOFF_MUL_arg5(agent, 0, 1, 2, 3, 6);").unwrap();
        assert_eq!(call.arguments, ["0", "1", "2", "3", "6.0"]);
        assert!(parse("macros::ATK_SET_SHIELD_SETOFF_MUL_arg5(agent, 0, 1, 2, -3, 6);").is_none());
        assert!(parse("macros::FILL_SCREEN_MODEL_COLOR(agent, 1, 10, 0.3, 0.3, 0.3, 0, 0, 0, 1, 1.3, EffectScreenLayer:*BOGUS, *EFFECT_SCREEN_PRIO_FINAL);").is_none());
        // The corpus request omits all seven operands; never infer them.
        assert!(
            parse("EffectModule::request_post_effect_line_circle(agent.module_accessor);")
                .is_none()
        );
        let complete = "EffectModule::request_post_effect_line_circle(agent.module_accessor, Hash40::new(\"effect\"), Hash40::new(\"top\"), smash::phx::simd::Vector2::new(1.0, 2.0), smash::phx::simd::Vector3::new(0.0, 0.0, 0.0), true, 1.0, 0.0);";
        let call = parse(complete).unwrap();
        assert_eq!(parse(&call.source()), Some(call));
        assert!(parse(&complete.replace("Vector2::new(1.0, 2.0)", "Vector2::new(1.0)")).is_none());
    }

    #[test]
    fn corpus_finish_and_damage_calls_retain_every_imported_operand() {
        // Exact statements from the pinned public corpus. Expected operands are independent
        // of parse/emission equality, which cannot detect a value lost by the first parse.
        for (origin, fighter, track, line, expected) in [
            ("inkling/AttackLw4.txt", "inkling", Track::Game, "AttackModule::set_ink_value(agent.module_accessor, 0, 150);", vec!["0", "150.0"]),
            ("lua2cpp_samus/samus/SpecialNHold.txt", "samus", Track::Expression, "macros::EFFECT_FOLLOW_LIGHT(agent, Hash40::new(\"samus_cshot_hold\"), Hash40::new(\"armr\"), 7.98, -0.506, -0.251, -91.273, -1.797, 176.373, 1, true, 0.471, 0.471, 0.471, 20, 1, 1, 0);", vec!["Hash40::new(\"samus_cshot_hold\")", "Hash40::new(\"armr\")", "7.98", "-0.506", "-0.251", "-91.273", "-1.797", "176.373", "1", "true", "0.471", "0.471", "0.471", "20", "1", "1", "0"]),
            ("lua2cpp_gaogaen/gaogaen/SpecialAirHiFall.txt", "samus", Track::Expression, "macros::RUMBLE_HIT_STATUS(agent, Hash40::new(\"rbkind_attackl\"), 0);", vec!["Hash40::new(\"rbkind_attackl\")", "0"]),
            (
                "bayonetta_specialn_bullet/Move.txt",
                "bayonetta",
                Track::Game,
                "AttackModule::enable_safe_pos(agent.module_accessor);",
                vec![],
            ),
            (
                "brave_flash/Flash1.txt",
                "brave",
                Track::Game,
                "AttackModule::disable_tip(agent.module_accessor);",
                vec![],
            ),
            (
                "luigi/SpecialSDischarge.txt",
                "luigi",
                Track::Game,
                "AttackModule::set_attack_keep_rumble(agent.module_accessor, 0, true);",
                vec!["0", "true"],
            ),
            (
                "diddy/SpecialAirHiJump.txt",
                "diddy",
                Track::Game,
                "GroundModule::select_cliff_hangdata(agent.module_accessor, *FIGHTER_DIDDY_CLIFF_HANG_DATA_SPECIAL_HI as u32);",
                vec!["*FIGHTER_DIDDY_CLIFF_HANG_DATA_SPECIAL_HI as u32"],
            ),
            (
                "diddy/SpecialNShoot.txt",
                "diddy",
                Track::Game,
                "ArticleModule::set_visibility(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_GUN, Hash40::new(\"set_main\"), Hash40::new(\"group_main_default\"), ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));",
                vec!["*FIGHTER_DIDDY_GENERATE_ARTICLE_GUN", "Hash40::new(\"set_main\")", "Hash40::new(\"group_main_default\")", "ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL)"],
            ),
            (
                "brave/FinalAirStart.txt",
                "brave",
                Track::Game,
                "macros::REQ_FINAL_START_CAMERA(agent, Hash40::new(\"d04finalstart.nuanmb\"), true);",
                vec!["Hash40::new(\"d04finalstart.nuanmb\")", "true"],
            ),
            (
                "purin/Final.txt",
                "purin",
                Track::Game,
                "AttackModule::set_invalid_xlu(agent.module_accessor, 0, true);",
                vec!["0", "true"],
            ),
            (
                "edge/SpecialHi2.txt",
                "edge",
                Track::Game,
                "AttackModule::set_no_finish_camera_ex(agent.module_accessor, 6, true, false);",
                vec!["6", "true", "false"],
            ),
            (
                "jack/VisualScene02.txt",
                "jack",
                Track::Expression,
                "ModelModule::disable_gold_eye(agent.module_accessor);",
                vec![],
            ),
            (
                "pichu/AttackAirB.txt",
                "pichu",
                Track::Game,
                "macros::FT_ADD_DAMAGE(agent, 1.5);",
                vec!["1.5"],
            ),
            (
                "eflame/ThrowLw.txt",
                "eflame",
                Track::Game,
                "macros::FT_LEAVE_NEAR_OTTOTTO(agent, -2.5, 2.5);",
                vec!["-2.5", "2.5"],
            ),
            (
                "lucina/ThrowLw.txt",
                "lucina",
                Track::Game,
                "macros::FT_LEAVE_NEAR_OTTOTTO(agent, 2, 2);",
                vec!["2", "2"],
            ),
        ] {
            let call = parse(line).unwrap_or_else(|| panic!("{origin}: {line}"));
            assert_eq!(call.arguments, expected, "{origin}");
            assert!(call.source_line.is_none());
            assert_eq!(parse(&call.source()), Some(call.clone()));
            let category = if track == Track::Game {
                "game"
            } else {
                "expression"
            };
            let source = format!("unsafe extern \"C\" fn {category}_fixture(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {line}\n    }}\n}}");
            let parse_track = if track == Track::Game {
                crate::acmd::parse_acmd_script
            } else {
                crate::acmd::parse_expression_script
            };
            let parsed = parse_track(&source);
            assert!(
                matches!(&parsed.stmts[0], AcmdStmt::Excute(commands)
                if matches!(commands.as_slice(), [ExcuteStmt::NativeCall(imported)] if imported == &call)),
                "{origin}"
            );
            let imported = timeline::import_for_fighter(fighter, track, &parsed);
            let coverage = super::super::coverage::track_summary(&imported);
            assert_eq!(coverage.editable_command_count, 1, "{origin}");
            assert_eq!(
                coverage.preserved_command_count + coverage.opaque_source_line_count,
                0
            );
            let graph = MoveGraph {
                tracks: vec![imported],
                ..Default::default()
            };
            let restored = serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
            let compiled = timeline::compile(fighter, "fixture", &restored).unwrap();
            let emitted = if track == Track::Game {
                crate::acmd::preview_game_fn(&compiled[0].1, "fixture")
            } else {
                crate::acmd::preview_expression_fn(&compiled[0].1, "fixture")
            };
            syn::parse_file(&emitted).unwrap();
            assert!(emitted.contains(&call.source()), "{origin}: {emitted}");
            assert_eq!(
                serde_json::to_value(&parsed).unwrap(),
                serde_json::to_value(parse_track(&emitted)).unwrap()
            );

            // Paired rejections retain the source instead of borrowing the valid signature.
            let receiver = call.signature().unwrap().receiver;
            for rejected in [
                line.replacen(receiver, "other", 1),
                line.replacen(");", ", 0);", 1),
                format!("#[cfg(any())] {line}"),
                format!("{line} other();"),
            ] {
                assert!(parse(&rejected).is_none(), "{origin}: {rejected}");
                let rejected_source = source.replace(line, &rejected);
                let rejected_track =
                    timeline::import_for_fighter(fighter, track, &parse_track(&rejected_source));
                assert_eq!(
                    super::super::coverage::track_summary(&rejected_track).editable_command_count,
                    0,
                    "{origin}: {rejected}"
                );
            }
            for index in 0..call.arguments.len() {
                let mut rejected = call.clone();
                rejected.arguments[index] = "dynamic_value()".into();
                assert!(rejected.validate().is_err(), "{origin}: argument {index}");
                assert!(parse(&rejected.source()).is_none());
            }
        }
    }

    #[test]
    fn typed_native_palette_exports_compile_against_the_sdk() {
        use crate::move_nodes::test_support as fixture;
        let commands = (109..=113)
            .map(|kind| super::super::view::context_command(kind, "diddy"))
            .collect();
        let graph = MoveGraph {
            tracks: vec![fixture::track(
                Track::Game,
                vec![fixture::execute(1, commands)],
            )],
            ..Default::default()
        };
        let compiled = timeline::compile("diddy", "fixture", &graph).unwrap();
        let project = crate::acmd::build_mod_project_full(
            &[("diddy".into(), "fixture".into(), compiled[0].1.clone())],
            &[],
            &[],
            &[],
            "typed_native_golden",
        );
        let root = crate::scratch_dirs::app_storage_root().join("typed-native-golden");
        for file in &project.files {
            if file.rel_path.ends_with(".rs") {
                syn::parse_file(&file.contents).unwrap();
            }
            let dest = root.join(&file.rel_path);
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::write(dest, &file.contents).unwrap();
        }
        if std::env::var_os("VISIONARY_COMPILE_GOLDEN").is_some() {
            let output = std::process::Command::new("cargo")
                .args(["skyline", "build", "--release"])
                .env("CARGO_TARGET_DIR", root.join("target"))
                .env("CARGO_BUILD_JOBS", "2")
                .current_dir(&root)
                .output()
                .expect("cargo-skyline not runnable");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn cliff_and_article_visibility_reject_foreign_owners_and_incomplete_calls() {
        for line in [
            "GroundModule::select_cliff_hangdata(agent.module_accessor, *FIGHTER_DIDDY_CLIFF_HANG_DATA_SPECIAL_HI as u32);",
            "ArticleModule::set_visibility(agent.module_accessor, *FIGHTER_DIDDY_GENERATE_ARTICLE_GUN, Hash40::new(\"set_main\"), Hash40::new(\"group_main_default\"), ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));",
        ] {
            let call = parse(line).unwrap();
            assert!(call.validate_for_fighter(Some("diddy")).is_ok());
            for owner in [Some("mario"), Some("unknown"), None] {
                assert!(call.validate_for_fighter(owner).is_err(), "{owner:?}: {line}");
            }
        }
        let common = parse("GroundModule::select_cliff_hangdata(agent.module_accessor, *FIGHTER_CLIFF_HANG_DATA_DEFAULT as u32);").unwrap();
        assert!(common.validate_for_fighter(Some("mario")).is_ok());
        assert!(common.validate_for_fighter(Some("unknown")).is_err());

        // The dump omits every operand here. It must stay verbatim, even beside typed calls.
        let source = r#"unsafe extern "C" fn game_flash1(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::disable_tip(agent.module_accessor);
        ArticleModule::set_visibility(agent.module_accessor);
        AttackModule::enable_safe_pos(agent.module_accessor);
        AttackModule::disable_tip(agent.module_accessor);
    }
}"#;
        let parsed = crate::acmd::parse_acmd_script(source);
        let imported = timeline::import_for_fighter("brave", Track::Game, &parsed);
        let counts = super::super::coverage::track_summary(&imported);
        assert_eq!(counts.editable_command_count, 3);
        assert_eq!(counts.preserved_command_count, 1);
        let graph = MoveGraph {
            tracks: vec![imported],
            ..Default::default()
        };
        let compiled = timeline::compile("brave", "flash1", &graph).unwrap();
        let emitted = crate::acmd::preview_game_fn(&compiled[0].1, "flash1");
        assert!(emitted.contains("ArticleModule::set_visibility(agent.module_accessor);"));
        assert_eq!(
            emitted
                .matches("AttackModule::disable_tip(agent.module_accessor);")
                .count(),
            2
        );
        assert_eq!(
            serde_json::to_value(parsed).unwrap(),
            serde_json::to_value(crate::acmd::parse_acmd_script(&emitted)).unwrap()
        );
    }

    #[test]
    fn incomplete_item_source_lines_keep_text_without_inventing_sdk_arguments() {
        for line in [
            "ItemModule::set_have_item_action(agent.module_accessor, *ITEM_SWING_ACTION_BEGIN);",
            "ItemModule::set_have_item_scale_anim(agent.module_accessor, 3, 1.1);",
            "ItemModule::pickup_item(agent.module_accessor, *ITEM_SIZE_LIGHT, *FIGHTER_HAVE_ITEM_WORK_TEMPORARY);",
            "ItemModule::use_item(agent.module_accessor, *FIGHTER_HAVE_ITEM_WORK_TEMPORARY);",
            "ItemModule::set_attach_item_visibility(agent.module_accessor, false);",
        ] {
            assert!(parse(line).is_none());
            let call = parse_source_line(line).unwrap();
            assert_eq!(call.source(), line);
            assert_eq!(call.signature().unwrap().caption, "Opaque source line");
            assert!(call.arguments.is_empty());
            let source = format!("unsafe extern \"C\" fn game_test(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {line}\n    }}\n}}");
            let parsed = crate::acmd::parse_acmd_script(&source);
            let track = timeline::import_for_fighter("mario", Track::Game, &parsed);
            assert!(matches!(track.nodes[0].kind, NodeKind::Execute { .. }));
            assert_eq!(
                super::super::coverage::track_summary(&track).editable_command_count,
                0
            );
            let graph = MoveGraph {
                tracks: vec![track],
                ..Default::default()
            };
            let restored = serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
            let compiled = timeline::compile("mario", "test", &restored).unwrap();
            let emitted = crate::acmd::preview_game_fn(&compiled[0].1, "test");
            assert!(emitted.contains(line));
            assert_eq!(
                serde_json::to_value(parsed).unwrap(),
                serde_json::to_value(crate::acmd::parse_acmd_script(&emitted)).unwrap()
            );
            assert!(super::super::live_support::source_only_tracks(&graph)
                .next()
                .is_some());
            let mut forged = call;
            forged.arguments.push("0".into());
            assert!(forged.validate().is_err());
        }
        for line in [
            "ItemModule::set_have_item_action(agent.module_accessor, *ITEM_SWING_ACTION_BEGIN, 1.0, 0);",
            "ItemModule::set_have_item_action(other, *ITEM_SWING_ACTION_BEGIN);",
            "ItemModule::set_have_item_action(agent.module_accessor, other());",
            "#[cfg(any())] ItemModule::set_have_item_action(agent.module_accessor, *ITEM_SWING_ACTION_BEGIN);",
            "ItemModule::set_have_item_action(agent.module_accessor, *ITEM_SWING_ACTION_BEGIN); other();",
        ] {
            assert!(parse_source_line(line).is_none(), "{line}");
        }
    }

    #[test]
    fn common_method_calls_roundtrip_as_explicit_sdk_calls() {
        for (track, category, class, method) in [
            (
                Track::Game,
                "game",
                "L2CFighterAnimcmdGameCommon",
                "game_BatSwing1Common",
            ),
            (
                Track::Expression,
                "expression",
                "L2CFighterAnimcmdExpressionCommon",
                "expression_BindRumble",
            ),
            (
                Track::Effect,
                "effect",
                "L2CFighterAnimcmdEffectCommon",
                "effect_ScopeAirRapidCommon",
            ),
        ] {
            let line = format!("methodlua2cpp::{class}::{method}()();");
            let call = parse(&line).unwrap();
            assert_eq!(parse(&call.source()), Some(call.clone()));
            let source = format!("unsafe extern \"C\" fn {category}_test(agent: &mut L2CAgentBase) {{\n    {line}\n}}");
            let imported = match track {
                Track::Effect => crate::move_nodes::effects::import_for_fighter(
                    "mario",
                    &crate::acmd::parse_effect_script(&source),
                ),
                _ => {
                    let script = if track == Track::Game {
                        crate::acmd::parse_acmd_script(&source)
                    } else {
                        crate::acmd::parse_expression_script(&source)
                    };
                    timeline::import_for_fighter("mario", track, &script)
                }
            };
            assert!(matches!(
                imported.nodes[0].kind,
                NodeKind::BareCommand { .. }
            ));
            let graph = MoveGraph {
                tracks: vec![imported],
                ..Default::default()
            };
            let restored = serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
            let compiled = timeline::compile("mario", "test", &restored).unwrap();
            let emitted = match track {
                Track::Game => crate::acmd::preview_game_fn(&compiled[0].1, "test"),
                Track::Expression => crate::acmd::preview_expression_fn(&compiled[0].1, "test"),
                _ => crate::acmd::preview_effect_structure_fn(&compiled[0].1, "test"),
            };
            assert!(emitted.contains(&call.source()));
            assert!(syn::parse_file(&emitted).is_ok());
            let reparsed = match track {
                Track::Effect => crate::move_nodes::effects::import_for_fighter(
                    "mario",
                    &crate::acmd::parse_effect_script(&emitted),
                ),
                _ => {
                    let script = if track == Track::Game {
                        crate::acmd::parse_acmd_script(&emitted)
                    } else {
                        crate::acmd::parse_expression_script(&emitted)
                    };
                    timeline::import_for_fighter("mario", track, &script)
                }
            };
            assert_eq!(
                serde_json::to_value(&graph.tracks[0]).unwrap(),
                serde_json::to_value(reparsed).unwrap()
            );
            let attributed = source.replace(&line, &format!("#[cfg(any())]\n    {line}"));
            let parsed = if track == Track::Effect {
                crate::move_nodes::effects::import_for_fighter(
                    "mario",
                    &crate::acmd::parse_effect_script(&attributed),
                )
            } else {
                let script = if track == Track::Game {
                    crate::acmd::parse_acmd_script(&attributed)
                } else {
                    crate::acmd::parse_expression_script(&attributed)
                };
                timeline::import_for_fighter("mario", track, &script)
            };
            assert!(!parsed
                .nodes
                .iter()
                .any(|node| matches!(node.kind, NodeKind::BareCommand { .. })));
        }
        for invalid in [
            "methodlua2cpp::L2CFighterAnimcmdGameCommon::game_Unknown()();",
            "methodlua2cpp::L2CFighterAnimcmdGameCommon::game_BatSwing1Common(1)();",
            "methodlua2cpp::L2CFighterAnimcmdGameCommon::game_BatSwing1Common();",
            "methodlua2cpp::L2CFighterAnimcmdGameCommon::game_BatSwing1Common()(); injected();",
            "smash::lua2cpp::L2CFighterAnimcmdGameCommon_game_BatSwing1Common(agent);",
        ] {
            assert!(parse(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn effect_native_flag_slots_validate_owner_and_keep_guarded_structure() {
        let source = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_AERIAL_REQ_EFFECT) {
        if macros::is_excute(agent) {
            WorkModule::off_flag(agent.module_accessor, *FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_AERIAL_REQ_EFFECT);
        }
    }
}"#;
        let script = crate::acmd::parse_effect_script(source);
        let graph = MoveGraph {
            tracks: vec![crate::move_nodes::effects::import_for_fighter(
                "mario", &script,
            )],
            ..Default::default()
        };
        assert!(matches!(
            graph.tracks[0].nodes[0].kind,
            NodeKind::Branch { .. }
        ));
        assert_eq!(
            crate::move_nodes::coverage::track_summary(&graph.tracks[0]).preserved_command_count,
            0
        );
        let restored = serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
        let compiled = timeline::compile("mario", "test", &restored).unwrap();
        let emitted = crate::acmd::preview_effect_structure_fn(&compiled[0].1, "test");
        assert_eq!(
            serde_json::to_value(script).unwrap(),
            serde_json::to_value(crate::acmd::parse_effect_script(&emitted)).unwrap()
        );
        let call = parse("WorkModule::on_flag(agent.module_accessor, *FIGHTER_RYU_INSTANCE_WORK_ID_FLAG_FINAL_HIT_CANCEL);").unwrap();
        assert!(call.validate_for_fighter(Some("ken")).is_ok());
        assert!(call.validate_for_fighter(Some("mario")).is_err());
        assert!(call.validate_for_fighter(None).is_err());
        assert!(parse("WorkModule::on_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA);").is_none());
    }

    #[test]
    fn implicit_msc_receivers_roundtrip_as_editable_stack_macros() {
        for line in [
            "THROW_ITEM_arg3(*ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_ANGLE, *ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_SPEED, *ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_POWER);",
            "REQ_FINAL_START_CAMERA_arg3(Hash40::new(\"d04finalstart.nuanmb\"), false, false);",
            "physics(*MA_MSC_CMD_PHYSICS_START_CHARGE, 0.7, 0.9, -1, 0.5, 0.5, -1, Hash40::new(\"invalid\"));",
            "physics(*MA_MSC_CMD_PHYSICS_PH_SWING_STOP, true);",
            "shield(*MA_MSC_CMD_REFLECTOR, *COLLISION_KIND_REFLECTOR, 0, Hash40::new(\"top\"), 9, 0, 7, 7, 0, 7, 8, 2, 2.5, 80, false, 0.5, *FIGHTER_REFLECTOR_GROUP_HOMERUNBAT);",
        ] {
            let call = parse(line).unwrap();
            assert!(call.source().starts_with(&format!("{}(agent, ", call.callee)));
            assert_eq!(parse(&call.source()), Some(call.clone()));
            let source = format!("unsafe extern \"C\" fn expression_test(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {line}\n    }}\n}}");
            let script = crate::acmd::parse_expression_script(&source);
            assert!(matches!(script.stmts.as_slice(), [AcmdStmt::Excute(commands)]
                if matches!(commands.as_slice(), [ExcuteStmt::NativeCall(parsed)] if parsed == &call)));
            let graph = MoveGraph {
                tracks: vec![timeline::import(Track::Expression, &script)],
                ..Default::default()
            };
            assert!(matches!(graph.tracks[0].nodes[0].kind, NodeKind::Execute { .. }));
            let graph = serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
            let compiled = timeline::compile("mario", "test", &graph).unwrap();
            let emitted = crate::acmd::preview_expression_fn(&compiled[0].1, "test");
            assert!(emitted.contains(&call.source()));
            assert_eq!(serde_json::to_value(script).unwrap(),
                serde_json::to_value(crate::acmd::parse_expression_script(&emitted)).unwrap());
            let attributed = source.replace(line, &format!("#[cfg(any())]\n        {line}"));
            let parsed = crate::acmd::parse_expression_script(&attributed);
            assert!(!matches!(parsed.stmts.as_slice(), [AcmdStmt::Excute(commands)]
                if matches!(commands.as_slice(), [ExcuteStmt::NativeCall(_)])));
        }
        let named = "unsafe extern \"C\" fn game_test(agent: &mut L2CAgentBase) {\n    if macros::is_excute(agent) {\n        ATK_SET_SHIELD_SETOFF_MUL2(ID=3, ShieldstunMul=3);\n    }\n}";
        let parsed = crate::acmd::parse_acmd_script(named);
        assert!(
            matches!(parsed.stmts.as_slice(), [AcmdStmt::Excute(commands)] if matches!(commands.as_slice(), [ExcuteStmt::AttackMod { .. }]))
        );
        let emitted = crate::acmd::preview_game_fn(&parsed, "test");
        assert_eq!(
            serde_json::to_value(&parsed).unwrap(),
            serde_json::to_value(crate::acmd::parse_acmd_script(&emitted)).unwrap()
        );
        for invalid in [
            "ATK_SET_SHIELD_SETOFF_MUL2(ShieldstunMul=3, ID=3);",
            "ATK_SET_SHIELD_SETOFF_MUL2(ID=3, ShieldstunMul=injected());",
            "ATK_SET_SHIELD_SETOFF_MUL2(ID=3, ShieldstunMul=3, Extra=0);",
            "THROW_ITEM_arg3(*ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_ANGLE);",
            "physics(*MA_MSC_CMD_PHYSICS_START_CHARGE, 0.7);",
            "physics(*MA_MSC_CMD_PHYSICS_PH_SWING_STOP, 1);",
            "physics(*MA_MSC_CMD_CAMERA_CAM_RESET);",
            "physics(agent, *MA_MSC_CMD_PHYSICS_PH_SWING_STOP, true);",
            "other::physics(*MA_MSC_CMD_PHYSICS_PH_SWING_STOP, true);",
            "physics!(*MA_MSC_CMD_PHYSICS_PH_SWING_STOP, true);",
            "physics!(other, *MA_MSC_CMD_PHYSICS_PH_SWING_STOP, true);",
            "let enabled = physics(*MA_MSC_CMD_PHYSICS_PH_SWING_STOP, true);",
            "camera!(agent, *MA_MSC_CMD_CAMERA_CAM_RECT, 1, 2, 3);",
            "search!(agent, *MA_MSC_CMD_SEARCH_SET_OPPONENT, 0, 0, *COLLISION_PROPERTY_MASK_REFLECT, *COLLISION_TARGET_PROPERTY);",
        ] {
            assert!(parse(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn sdk_hash_and_byte_arguments_convert_without_changing_slots() {
        let source = "VisibilityModule::set_status_default_int64(agent.module_accessor, Hash40::new(\"mantle\"), Hash40::new(\"mantle_wing\"));";
        let call = parse(source).unwrap();
        assert_eq!(
            call.arguments,
            [
                "Hash40::new(\"mantle\").hash as i64",
                "Hash40::new(\"mantle_wing\").hash as i64"
            ]
        );
        assert_eq!(parse(&call.source()), Some(call));
        let kind = Byte(&["ATTACH_ITEM_GROUP_BACK"]);
        assert_eq!(
            kind.normalize("*ATTACH_ITEM_GROUP_BACK"),
            Some("*ATTACH_ITEM_GROUP_BACK as u8".into())
        );
        assert!(kind.normalize("*FIGHTER_STATUS_KIND_WAIT").is_none());
        assert!(kind.normalize("256").is_none());
        assert!(kind.normalize("256 as u8").is_none());
        assert!(HashInteger.normalize("other().hash as i64").is_none());
        assert!(
            parse("ItemModule::set_attach_item_visibility(agent.module_accessor, false);")
                .is_none()
        );
    }

    #[test]
    fn weapon_work_reads_and_force_reaction_reject_wrong_arity_and_slots() {
        // Positive statements in CORPUS_LINES are lifted from the vanilla dump.
        for line in CORPUS_LINES
            .iter()
            .filter(|line| line.contains("WEAPON_") && line.starts_with("WorkModule::get_float("))
        {
            assert!(parse(line).is_some());
            assert!(
                parse(&line.replace("agent.module_accessor,", "agent.module_accessor, 0,"))
                    .is_none()
            );
            assert!(parse(&line.replace("WorkModule::get_float", "WorkModule::get_int")).is_none());
            assert!(
                parse(&line.replace("agent.module_accessor", "other.module_accessor")).is_none()
            );
        }
        assert!(
            parse("AttackModule::set_force_reaction(agent.module_accessor, 0, true);").is_none()
        );
        assert!(
            parse("AttackModule::set_force_reaction(agent.module_accessor, 0, true, 1);").is_none()
        );
        assert!(parse("AttackModule::add_power(agent.module_accessor, 0, false);").is_none());
    }

    #[test]
    fn effect_native_reads_keep_bare_placement_and_bindings() {
        let source = r#"unsafe extern "C" fn effect_appealsl(agent: &mut L2CAgentBase) {
    WorkModule::get_float(agent.module_accessor, *FIGHTER_INKLING_INSTANCE_WORK_ID_FLOAT_INK_R);
    if macros::is_excute(agent) {
        notify_event_msc_cmd!(agent, Hash40::new_raw(0x1f20a9d549), false);
    }
}"#;
        let script = crate::acmd::parse_effect_script(source);
        assert!(
            matches!(script.stmts.as_slice(), [crate::data::EffectStmt::BareNativeCall(_), crate::data::EffectStmt::Excute(commands)] if matches!(commands.as_slice(), [crate::data::EffectMacro::NativeCall(_)]))
        );
        let graph = MoveGraph {
            tracks: vec![crate::move_nodes::effects::import_for_fighter(
                "inkling", &script,
            )],
            ..Default::default()
        };
        let graph = serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
        let compiled = timeline::compile("inkling", "appealsl", &graph).unwrap();
        let emitted = crate::acmd::preview_effect_structure_fn(&compiled[0].1, "appealsl");
        assert_eq!(
            serde_json::to_value(script).unwrap(),
            serde_json::to_value(crate::acmd::parse_effect_script(&emitted)).unwrap()
        );
        let bound = parse("let restart_frame = WorkModule::get_float(agent.module_accessor, *FIGHTER_STATUS_ATTACK_WORK_FLOAT_SMASH_RESTART_FRAME);").unwrap();
        assert_eq!(bound.binding.as_deref(), Some("restart_frame"));
        assert_eq!(parse(&bound.source()), Some(bound));
        assert!(parse("WorkModule::get_float(agent.module_accessor, *FIGHTER_STATUS_JUMP_FLAG_GIMMICK_SPRING_JUMP_FROM_RING);").is_none());
    }

    #[test]
    fn native_signatures_keep_argument_slots_and_editable_roundtrip() {
        for line in CORPUS_LINES {
            let call = parse(line).unwrap();
            assert_eq!(parse(&call.source()).as_ref(), Some(&call));
            let source = format!("unsafe extern \"C\" fn game_test(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {line}\n    }}\n}}");
            let script = crate::acmd::parse_acmd_script(&source);
            assert!(
                matches!(script.stmts.as_slice(), [AcmdStmt::Excute(commands)] if matches!(commands.as_slice(), [command] if !matches!(command, ExcuteStmt::Raw(_))))
            );
            let track = timeline::import(Track::Game, &script);
            assert!(matches!(track.nodes[0].kind, NodeKind::Execute { .. }));
            let graph = MoveGraph {
                tracks: vec![track],
                ..Default::default()
            };
            let graph = serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
            let compiled = timeline::compile("mario", "test", &graph).unwrap();
            let emitted = crate::acmd::preview_game_fn(&compiled[0].1, "test");
            assert_eq!(emitted.matches(&call.source()).count(), 1);
            let mut edited = call.clone();
            if let Some(argument) = edited.arguments.first_mut() {
                *argument = "{ injected(); 0 }".into();
                assert!(edited.validate().is_err());
            }
        }
        assert!(
            parse("ItemModule::pickup_item(agent.module_accessor, *ITEM_SIZE_LIGHT);").is_none()
        );
        assert!(parse("PostureModule::scale(agent.module_accessor, 4, 0);").is_none());
        assert!(parse("ArticleModule::change_motion(agent.module_accessor, *FIGHTER_STATUS_KIND_WAIT, Hash40::new(\"wait\"), false, -1.0);").is_none());
        assert!(parse("lua_bind::FighterCutInManager::set_throw_finish_offset(singletons::FighterCutInManager(), Vector3f{x: 1.0, x: 2.0, z: 0.0});").is_none());
        assert!(parse("notify_event_msc_cmd!(agent, Hash40::new_raw(0x2127e37c07), *FIGHTER_STATUS_KIND_WAIT);").is_none());
        assert!(
            parse("notify_event_msc_cmd!(agent, Hash40::new_raw(0x26769bd1de), 1, 2);").is_none()
        );
        assert!(parse("let agent = MotionModule::rate(agent.module_accessor);").is_none());
        assert!(parse("let x = macros::LAST_EFFECT_SET_RATE(agent, 1.0);").is_none());
        let mut swapped = parse(CORPUS_LINES[0]).unwrap();
        swapped.arguments.swap(2, 3);
        assert!(swapped.validate().is_err());
        for value in ["#[cfg(any())] 1", "1f64", "*FIGHTER_STATUS_KIND_WAIT"] {
            assert!(!ArgumentKind::Integer(&[]).accepts(value));
        }
        assert!(!Hash.accepts("Hash40::<()>::new(\"rumble\")"));
        assert!(!Hash.accepts("Hash40::new(#[cfg(any())] \"rumble\")"));
        assert!(parse(&CORPUS_LINES[1].replace("SSCOPE_CHARGE", "UNRELATED")).is_none());
        assert!(parse(&CORPUS_LINES[1].replace("agent,", "other,")).is_none());
        assert!(parse(&CORPUS_LINES[1].replace(");", ", 0);")).is_none());
    }
}
