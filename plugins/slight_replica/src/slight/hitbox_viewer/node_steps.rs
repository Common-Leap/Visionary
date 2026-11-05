//! Blocks from the node editor that the game's own script never does.
//!
//! The editor sends each one as an injection in `CAT_NODE_STEP`. An If in the editor arrives as a
//! [`LiveGuard`], checked at the moment the block is due, the way the script's own `if` would be.
use super::LuaArg;
use smash::app::lua_bind::{
    ArticleModule, AttackModule, ControlModule, HitModule, ItemModule, JostleModule, KineticModule,
    PostureModule, SlowModule, StatusModule, VisibilityModule, WorkModule,
};
use smash::app::BattleObjectModuleAccessor as Boma;
use smash::lib::lua_const::*;

/// Must equal the editor's wire value.
pub const CAT_NODE_STEP: u8 = 37;
/// `ArticleModule::generate_article`, by article id. Must equal the editor's wire value.
pub const CAT_ARTICLE_GENERATE: u8 = 38;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveGuard {
    Grounded,
    Airborne,
    ItemHeld,
    FacingLeft,
    AttackHit,
    AttackShield,
    Stick {
        vertical: bool,
        op: i8,
        value: f32,
        facing: bool,
    },
    Button(String),
    Flag {
        symbol: u64,
        expected: bool,
    },
    Int {
        symbol: u64,
        op: i8,
        value: i32,
    },
    Float {
        symbol: u64,
        op: i8,
        value: f32,
    },
    Status {
        symbol: u64,
    },
    Article {
        symbol: u64,
    },
    Value {
        chart: String,
        var: String,
        op: i8,
        value: f32,
    },
    All(Vec<LiveGuard>),
    Any(Vec<LiveGuard>),
    Not(Box<LiveGuard>),
}

/// The editor hashes names in lower case.
fn name_hash(name: &str) -> u64 {
    smash::hash40(&name.to_ascii_lowercase())
}

fn compare(op: i8, left: f32, right: f32) -> bool {
    match op {
        -2 => left < right,
        -1 => left <= right,
        0 => left == right,
        1 => left >= right,
        2 => left > right,
        _ => left != right,
    }
}

/// Work ids by the hash of their source name. Fighter-owned ids come from the registry.
unsafe fn work_id(
    boma: *mut Boma,
    symbol: u64,
    kind: crate::slight::gimmicks::registry::IdentifierKind,
) -> Option<i32> {
    use crate::slight::gimmicks::registry::{fighter_name, find_value};
    let fighter = fighter_name(smash::app::utility::get_kind(&mut *boma))?;
    let found =
        common_flag(symbol).or_else(|| find_value(fighter, kind, |name| name_hash(name) == symbol));
    if found.is_none() {
        crate::slight::diag::note(format!("node step: unknown game value {symbol:#x}"));
    }
    found
}

pub unsafe fn passes(boma: *mut Boma, guard: &LiveGuard) -> bool {
    use crate::slight::gimmicks::registry::IdentifierKind as Id;
    match guard {
        LiveGuard::Grounded => StatusModule::situation_kind(boma) == *SITUATION_KIND_GROUND,
        LiveGuard::Airborne => StatusModule::situation_kind(boma) == *SITUATION_KIND_AIR,
        LiveGuard::ItemHeld => ItemModule::is_have_item(boma, 0),
        LiveGuard::FacingLeft => PostureModule::lr(boma) < 0.0,
        LiveGuard::AttackHit => AttackModule::is_infliction(boma, *COLLISION_KIND_MASK_HIT),
        LiveGuard::AttackShield => AttackModule::is_infliction(boma, *COLLISION_KIND_MASK_SHIELD),
        LiveGuard::Stick {
            vertical,
            op,
            value,
            facing,
        } => {
            let mut stick = if *vertical {
                ControlModule::get_stick_y(boma)
            } else {
                ControlModule::get_stick_x(boma)
            };
            if *facing && !*vertical {
                stick *= PostureModule::lr(boma);
            }
            compare(*op, stick, *value)
        }
        LiveGuard::Button(button) => {
            let id = match button.as_str() {
                "Attack" => *CONTROL_PAD_BUTTON_ATTACK,
                "Special" => *CONTROL_PAD_BUTTON_SPECIAL,
                "Jump" => *CONTROL_PAD_BUTTON_JUMP,
                "Guard" => *CONTROL_PAD_BUTTON_GUARD,
                "Grab" => *CONTROL_PAD_BUTTON_CATCH,
                _ => return false,
            };
            ControlModule::check_button_on(boma, id)
        }
        LiveGuard::Flag { symbol, expected } => match work_id(boma, *symbol, Id::WorkFlag) {
            Some(id) => WorkModule::is_flag(boma, id) == *expected,
            None => false,
        },
        LiveGuard::Int { symbol, op, value } => match work_id(boma, *symbol, Id::WorkInt) {
            Some(id) => compare(*op, WorkModule::get_int(boma, id) as f32, *value as f32),
            None => false,
        },
        LiveGuard::Float { symbol, op, value } => match work_id(boma, *symbol, Id::WorkFloat) {
            Some(id) => compare(*op, WorkModule::get_float(boma, id), *value),
            None => false,
        },
        LiveGuard::Status { symbol } => match work_id(boma, *symbol, Id::FighterStatus) {
            Some(id) => StatusModule::status_kind(boma) == id,
            None => false,
        },
        LiveGuard::Article { symbol } => match work_id(boma, *symbol, Id::Article) {
            Some(id) => ArticleModule::is_exist(boma, id),
            None => false,
        },
        LiveGuard::Value {
            chart,
            var,
            op,
            value,
        } => {
            let boid = (*boma).battle_object_id;
            crate::slight::gimmicks::live_behavior::chart_value(boid, chart, var)
                .is_some_and(|current| compare(*op, current, *value))
        }
        LiveGuard::All(all) => all.iter().all(|guard| passes(boma, guard)),
        LiveGuard::Any(any) => any.iter().any(|guard| passes(boma, guard)),
        LiveGuard::Not(inner) => !passes(boma, inner),
    }
}

fn int(arg: Option<&LuaArg>) -> Option<i32> {
    match arg? {
        LuaArg::Int(value) => Some(*value as i32),
        _ => None,
    }
}

/// Performs one step. Script macros run through `agent`, whose stack already holds `args`.
pub unsafe fn run(lua_state: u64, boma: *mut Boma, command: &str, args: &[LuaArg]) -> bool {
    use crate::slight::gimmicks::registry::IdentifierKind as Id;
    if let Some(target) = command.strip_prefix("event:") {
        let Some((chart, name)) = target.split_once(':') else {
            return false;
        };
        if chart == "main" {
            crate::slight::gimmicks::dispatch_named_from_module(boma, name);
        } else {
            crate::slight::gimmicks::dispatch_named_chart_from_module(boma, chart, name);
        }
        return true;
    }
    match command {
        "SET_SPEED" => smash::app::sv_animcmd::SET_SPEED(lua_state),
        "ADD_SPEED_NO_LIMIT" => smash::app::sv_animcmd::ADD_SPEED_NO_LIMIT(lua_state),
        "SET_AIR" => smash::app::sv_animcmd::SET_AIR(lua_state),
        "REVERSE_LR" => smash::app::sv_animcmd::REVERSE_LR(lua_state),
        "FT_MOTION_RATE" => smash::app::sv_animcmd::FT_MOTION_RATE(lua_state),
        "clear_speed_all" => {
            KineticModule::clear_speed_all(boma);
        }
        "set_whole" => {
            let visible = matches!(args.first(), Some(LuaArg::Bool(true)));
            VisibilityModule::set_whole(boma, visible);
        }
        "generate_article" => {
            let Some(id) = int(args.first()) else {
                return false;
            };
            let made = ArticleModule::generate_article(boma, id, false, -1);
            crate::slight::diag::note(format!(
                "node step spawned {id}: handle {made:#x}, out={}",
                ArticleModule::is_exist(boma, id)
            ));
        }
        "remove_article" => {
            let Some(id) = int(args.first()) else {
                return false;
            };
            ArticleModule::remove_exist(
                boma,
                id,
                smash::app::ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL),
            );
        }
        "article_visibility" => {
            let Some(id) = int(args.first()) else {
                return false;
            };
            let visible = matches!(args.get(1), Some(LuaArg::Bool(true)));
            ArticleModule::set_visibility_whole(
                boma,
                id,
                visible,
                smash::app::ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL),
            );
        }
        "hit_all_normal" => {
            HitModule::set_status_all(boma, smash::app::HitStatus(*HIT_STATUS_NORMAL), 0);
        }
        "start_cutin" => {
            smash::app::sv_animcmd::FT_START_CUTIN(lua_state);
        }
        "flash_eye" => {
            smash::app::sv_animcmd::START_INFO_FLASH_EYE(lua_state);
        }
        "jostle" => {
            JostleModule::set_status(boma, matches!(args.first(), Some(LuaArg::Bool(true))));
        }
        "shoot_article" => {
            let Some(id) = int(args.first()) else {
                return false;
            };
            ArticleModule::shoot(
                boma,
                id,
                smash::app::ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL),
                false,
            );
        }
        "apply_throw" => {
            let mut agent = smash::lib::L2CAgent::new(lua_state);
            let held = |slot: i32| LuaArg::Int(WorkModule::get_int64(boma, slot) as i64);
            for arg in [
                LuaArg::Int(*FIGHTER_ATTACK_ABSOLUTE_KIND_THROW as i64),
                LuaArg::Hash(smash::hash40("throw")),
                held(*FIGHTER_STATUS_THROW_WORK_INT_TARGET_OBJECT),
                held(*FIGHTER_STATUS_THROW_WORK_INT_TARGET_HIT_GROUP),
                held(*FIGHTER_STATUS_THROW_WORK_INT_TARGET_HIT_NO),
            ] {
                let mut value = arg.to_l2c();
                agent.push_lua_stack(&mut value);
            }
            smash::app::sv_animcmd::ATK_HIT_ABS(lua_state);
        }
        "camera_zoom_out" => {
            smash::app::sv_animcmd::CAM_ZOOM_OUT(lua_state);
        }
        "slow_whole" => {
            let (Some(multiplier), Some(frames)) = (int(args.first()), int(args.get(1))) else {
                return false;
            };
            SlowModule::set_whole(boma, multiplier as u8, frames);
        }
        "slow_clear" => {
            SlowModule::clear_whole(boma);
        }
        "enable_term" | "unable_term" => {
            let Some(LuaArg::Hash(symbol)) = args.first() else {
                return false;
            };
            let Some(id) = transition_term(*symbol) else {
                crate::slight::diag::note(format!("node step: unknown transition {symbol:#x}"));
                return false;
            };
            if command == "enable_term" {
                WorkModule::enable_transition_term(boma, id);
            } else {
                WorkModule::unable_transition_term(boma, id);
            }
        }
        "on_flag" | "off_flag" => {
            let Some(LuaArg::Hash(symbol)) = args.first() else {
                return false;
            };
            let Some(id) = work_id(boma, *symbol, Id::WorkFlag) else {
                return false;
            };
            if command == "on_flag" {
                WorkModule::on_flag(boma, id);
            } else {
                WorkModule::off_flag(boma, id);
            }
        }
        _ => return false,
    }
    true
}

/// The flags every fighter shares, which the fighter registry does not list.
fn common_flag(symbol: u64) -> Option<i32> {
    macro_rules! table { ($($name:ident,)*) => {
        $(if symbol == name_hash(stringify!($name)) { return Some(*$name); })*
    }; }
    table! {
        FIGHTER_INSTANCE_WORK_ID_FLAG_BACKSHIELD_INVISIBLE,
        FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_CAPTURE_PULLED_SE,
        FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA,
        FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FURAFURA_VOICE_FINAL,
        FIGHTER_INSTANCE_WORK_ID_FLAG_FORCE_LOUPE,
        FIGHTER_INSTANCE_WORK_ID_FLAG_NO_SPEED_OPERATION_CHK,
        FIGHTER_INSTANCE_WORK_ID_FLAG_RABBIT_CAP,
        FIGHTER_INSTANCE_WORK_ID_FLAG_RESULT_FIGHTER_SINGLE,
        FIGHTER_INSTANCE_WORK_ID_FLAG_REVERSE_LR_FINISH_CAMERA_THROW_ORBIT,
        FIGHTER_STATUS_AIR_LASSO_FLAG_CHECK,
        FIGHTER_STATUS_AIR_LASSO_FLAG_HANG_IMMIDIATE,
        FIGHTER_STATUS_AIR_LASSO_FLAG_LANDING,
        FIGHTER_STATUS_ATTACH_WALL_FLAG_MOT_STOP,
        FIGHTER_STATUS_ATTACK_AIR_FLAG_ENABLE_LANDING,
        FIGHTER_STATUS_ATTACK_AIR_FLAG_KEEP_AIR,
        FIGHTER_STATUS_ATTACK_AIR_FLAG_LANDING_CLEAR_SPEED,
        FIGHTER_STATUS_ATTACK_FLAG_100_CONTINUE_CHECK,
        FIGHTER_STATUS_ATTACK_FLAG_ENABLE_100,
        FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO,
        FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO_INPUT,
        FIGHTER_STATUS_ATTACK_FLAG_ENABLE_NO_HIT_COMBO,
        FIGHTER_STATUS_ATTACK_FLAG_ENABLE_RESTART,
        FIGHTER_STATUS_ATTACK_FLAG_SMASH_SMASH_HOLD_TO_ATTACK,
        FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD,
        FIGHTER_STATUS_CATCH_FLAG_CATCH_WAIT,
        FIGHTER_STATUS_DASH_FLAG_TURN_DASH,
        FIGHTER_STATUS_DOWN_FLAG_PREV_STATUS_DOWN_EAT,
        FIGHTER_STATUS_ESCAPE_AIR_FLAG_SLIDE_ENABLE_CONTROL,
        FIGHTER_STATUS_ESCAPE_AIR_FLAG_SLIDE_ENABLE_GRAVITY,
        FIGHTER_STATUS_GENESIS_SHOOT_FLAG_REQUEST_RADIATING,
        FIGHTER_STATUS_GENESIS_SHOOT_FLAG_TURN,
        FIGHTER_STATUS_ITEM_GRASS_PULL_FLAG_STTUS_END_IF_PICKUP_FAILED,
        FIGHTER_STATUS_ITEM_LIFT_WORK_FLAG_STOP,
        FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_FFLOWER_SHOOT,
        FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_AERIAL_REQ_EFFECT,
        FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_AERIAL_REQ_SOUND,
        FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_SHOOT,
        FIGHTER_STATUS_ITEM_SPECIALFLAG_HOIST_WORK_FLAG_HOIST_START,
        FIGHTER_STATUS_ITEM_SWING_WORK_FLAG_START_SMASH_HOLD,
        FIGHTER_STATUS_JUMP_FLAG_GIMMICK_SPRING_JUMP_FROM_RING,
        FIGHTER_STATUS_JUMP_FLY_NEXT,
        FIGHTER_STATUS_RUN_BRAKE_FLAG_STOP,
        FIGHTER_STATUS_RUN_BRAKE_FLAG_TURN_RUN,
        FIGHTER_STATUS_SQUAT_FLAG_REQUEST_SQUAT_SE,
        FIGHTER_STATUS_STOP_CEIL_FLAG_FALL,
        FIGHTER_STATUS_SUPER_JUMP_PUNCH_FLAG_MOVE_TRANS,
        FIGHTER_STATUS_SUPER_JUMP_PUNCH_FLAG_REVERSE_LR,
        FIGHTER_STATUS_THROW_FLAG_START_AIR,
        FIGHTER_STATUS_THROW_FLAG_STOP,
        FIGHTER_STATUS_TURN_RUN_FLAG_STOP,
        FIGHTER_STATUS_TURN_RUN_FLAG_TO_RUN,
        FIGHTER_STATUS_VISUAL_SCENE_FLAG_ABS_SET,
        FIGHTER_STATUS_VISUAL_SCENE_FLAG_END_EXIT,
        FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_ATTACK_DISABLE_MINI_JUMP_ATTACK,
        FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_CHANGE_STATUS_DLAY_MOTION,
        FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_GRAVITY_STABLE_UNABLE,
        FIGHTER_STATUS_WORK_ID_FLAG_RESERVE_LANDING_ATTACK_AIR_SQUAT,
    }
    None
}

/// The cancel options the block list offers.
fn transition_term(symbol: u64) -> Option<i32> {
    macro_rules! table { ($($name:ident,)*) => {
        $(if symbol == name_hash(stringify!($name)) { return Some(*$name); })*
    }; }
    table! {
        FIGHTER_DIDDY_STATUS_SPECIAL_S_TRANSITION_TERM_ID_GROUND,
        FIGHTER_LUIGI_STATUS_SPECIAL_S_RAM_TRANSITION_TERM_ID_GROUND,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_HI3,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_LW3,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_LW4_START,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_S3,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_S4_START,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ITEM_THROW,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_TURN_DASH,
        FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN,
        FIGHTER_STATUS_TRANSITION_TERM_ID_LANDING,
    }
    None
}
