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
    IntNamed {
        symbol: u64,
        op: i8,
        value: u64,
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
    Item {
        symbol: u64,
    },
    AnimationChanging,
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
    let found = find_id(boma, symbol, kind);
    if found.is_none() {
        crate::slight::diag::note(format!("node step: unknown game value {symbol:#x}"));
    }
    found
}

/// [`work_id`] without the log, for lookups that try several tables.
unsafe fn find_id(
    boma: *mut Boma,
    symbol: u64,
    kind: crate::slight::gimmicks::registry::IdentifierKind,
) -> Option<i32> {
    use crate::slight::gimmicks::registry::{fighter_name, find_value};
    let fighter = fighter_name(smash::app::utility::get_kind(&mut *boma))?;
    common_flag(symbol).or_else(|| find_value(fighter, kind, |name| name_hash(name) == symbol))
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
        LiveGuard::IntNamed { symbol, op, value } => {
            match (work_id(boma, *symbol, Id::WorkInt), named_const(*value)) {
                (Some(id), Some(value)) => {
                    compare(*op, WorkModule::get_int(boma, id) as f32, value as f32)
                }
                _ => false,
            }
        }
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
        LiveGuard::Item { symbol } => match work_id(boma, *symbol, Id::Item) {
            Some(id) => ItemModule::get_have_item_kind(boma, 0) == id,
            None => false,
        },
        LiveGuard::AnimationChanging => smash::app::lua_bind::MotionModule::is_changing(boma),
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
    if generic(lua_state, boma, command, args).unwrap_or(false) {
        return true;
    }
    if let Some(target) = command.strip_prefix("eventv:") {
        let mut parts = target.splitn(3, ':');
        let (Some(chart), Some(name), Some(fields)) = (parts.next(), parts.next(), parts.next())
        else {
            return false;
        };
        use crate::slight::gimmicks::ChartEventValue as V;
        let values: Vec<(&str, V)> = fields
            .split(',')
            .filter(|field| !field.is_empty())
            .zip(args)
            .filter_map(|(field, arg)| {
                Some((
                    field,
                    match arg {
                        LuaArg::Int(value) => V::Int(*value as i32),
                        LuaArg::Num(value) => V::Float(*value),
                        LuaArg::Bool(value) => V::Flag(*value),
                        _ => return None,
                    },
                ))
            })
            .collect();
        crate::slight::gimmicks::dispatch_named_chart_values_from_module(
            boma, chart, name, &values,
        );
        return true;
    }
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
        "SET_SPEED" => {
            push(lua_state, boma, args);
            smash::app::sv_animcmd::SET_SPEED(lua_state);
        }
        "ADD_SPEED_NO_LIMIT" => {
            push(lua_state, boma, args);
            smash::app::sv_animcmd::ADD_SPEED_NO_LIMIT(lua_state);
        }
        "SET_AIR" => smash::app::sv_animcmd::SET_AIR(lua_state),
        "REVERSE_LR" => smash::app::sv_animcmd::REVERSE_LR(lua_state),
        "FT_MOTION_RATE" => {
            push(lua_state, boma, args);
            smash::app::sv_animcmd::FT_MOTION_RATE(lua_state);
        }
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
            use crate::slight::gimmicks::registry;
            let fighter = registry::fighter_name(smash::app::utility::get_kind(&mut *boma));
            if let Some(fighter) = fighter {
                if let Some(symbol) = registry::symbol_for_value(
                    fighter,
                    registry::IdentifierKind::Article,
                    id as u32,
                ) {
                    crate::slight::gimmicks::deflect_volume::remove_all(
                        (*boma).battle_object_id,
                        fighter,
                        symbol,
                    );
                }
            }
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
/// Most attack code never asks whether a cancel is allowed, so an Allow cancel block would do
/// nothing on its own. Once one has run in this move, take the player's input ourselves.
pub unsafe fn follow_cancels(boma: *mut Boma, symbols: &[u64]) {
    let ground = StatusModule::situation_kind(boma) == *SITUATION_KIND_GROUND;
    let cat1 = ControlModule::get_command_flag_cat(boma, 0);
    let pressed = |flags: &[i32]| flags.iter().any(|flag| cat1 & *flag != 0);
    let jumps_left = WorkModule::get_int(boma, *FIGHTER_INSTANCE_WORK_ID_INT_JUMP_COUNT)
        < WorkModule::get_int(boma, *FIGHTER_INSTANCE_WORK_ID_INT_JUMP_COUNT_MAX);
    for symbol in symbols {
        let Some(term) = transition_term(*symbol) else {
            continue;
        };
        // A later Block cancel in the same move turns it back off.
        if !WorkModule::is_enable_transition_term(boma, term) {
            continue;
        }
        let jump = *FIGHTER_PAD_CMD_CAT1_FLAG_JUMP;
        let jump_button = *FIGHTER_PAD_CMD_CAT1_FLAG_JUMP_BUTTON;
        let air_attacks = [
            *FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_N,
            *FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_AIR_N,
            *FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_AIR_F,
            *FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_AIR_B,
            *FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_AIR_HI,
            *FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_AIR_LW,
        ];
        let (fire, status) = match term {
            t if t == *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_JUMP_SQUAT_BUTTON => (
                ground && pressed(&[jump_button]),
                *FIGHTER_STATUS_KIND_JUMP_SQUAT,
            ),
            t if t == *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_JUMP_SQUAT => {
                (ground && pressed(&[jump]), *FIGHTER_STATUS_KIND_JUMP_SQUAT)
            }
            t if t == *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_JUMP_AERIAL_BUTTON => (
                !ground && jumps_left && pressed(&[jump_button]),
                *FIGHTER_STATUS_KIND_JUMP_AERIAL,
            ),
            t if t == *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_JUMP_AERIAL => (
                !ground && jumps_left && pressed(&[jump]),
                *FIGHTER_STATUS_KIND_JUMP_AERIAL,
            ),
            t if t == *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK => (
                ground && pressed(&[*FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_N]),
                *FIGHTER_STATUS_KIND_ATTACK,
            ),
            t if t == *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_AIR => (
                !ground && pressed(&air_attacks),
                *FIGHTER_STATUS_KIND_ATTACK_AIR,
            ),
            t if t == *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_N => (
                pressed(&[*FIGHTER_PAD_CMD_CAT1_FLAG_SPECIAL_N]),
                *FIGHTER_STATUS_KIND_SPECIAL_N,
            ),
            t if t == *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_S => (
                pressed(&[*FIGHTER_PAD_CMD_CAT1_FLAG_SPECIAL_S]),
                *FIGHTER_STATUS_KIND_SPECIAL_S,
            ),
            t if t == *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_HI => (
                pressed(&[*FIGHTER_PAD_CMD_CAT1_FLAG_SPECIAL_HI]),
                *FIGHTER_STATUS_KIND_SPECIAL_HI,
            ),
            t if t == *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_LW => (
                pressed(&[*FIGHTER_PAD_CMD_CAT1_FLAG_SPECIAL_LW]),
                *FIGHTER_STATUS_KIND_SPECIAL_LW,
            ),
            t if t == *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_GUARD_ON => (
                ground && ControlModule::check_button_on(boma, *CONTROL_PAD_BUTTON_GUARD),
                *FIGHTER_STATUS_KIND_GUARD_ON,
            ),
            t if t == *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ESCAPE => {
                if ground {
                    // Rolls forward and back are their own actions; a plain dodge is in place.
                    let forward = pressed(&[*FIGHTER_PAD_CMD_CAT1_FLAG_ESCAPE_F]);
                    let back = pressed(&[*FIGHTER_PAD_CMD_CAT1_FLAG_ESCAPE_B]);
                    (
                        forward || back || pressed(&[*FIGHTER_PAD_CMD_CAT1_FLAG_ESCAPE]),
                        if forward {
                            *FIGHTER_STATUS_KIND_ESCAPE_F
                        } else if back {
                            *FIGHTER_STATUS_KIND_ESCAPE_B
                        } else {
                            *FIGHTER_STATUS_KIND_ESCAPE
                        },
                    )
                } else {
                    (
                        pressed(&[*FIGHTER_PAD_CMD_CAT1_FLAG_AIR_ESCAPE]),
                        *FIGHTER_STATUS_KIND_ESCAPE_AIR,
                    )
                }
            }
            _ => continue,
        };
        if fire {
            StatusModule::change_status_request_from_script(boma, status, true);
            crate::slight::diag::note(format!("cancel into status {status}"));
            return;
        }
    }
}

fn transition_term(symbol: u64) -> Option<i32> {
    macro_rules! table { ($($name:ident,)*) => {
        $(if symbol == name_hash(stringify!($name)) { return Some(*$name); })*
    }; }
    table! {
        FIGHTER_DIDDY_STATUS_SPECIAL_S_TRANSITION_TERM_ID_GROUND,
        FIGHTER_LUIGI_STATUS_SPECIAL_S_RAM_TRANSITION_TERM_ID_GROUND,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_AIR,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_HI3,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_LW3,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_LW4_START,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_S3,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_S4_START,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ESCAPE,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_GUARD_ON,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ITEM_THROW,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_JUMP_AERIAL,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_JUMP_AERIAL_BUTTON,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_JUMP_SQUAT,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_JUMP_SQUAT_BUTTON,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_HI,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_LW,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_N,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_S,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_TURN_DASH,
        FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN,
        FIGHTER_STATUS_TRANSITION_TERM_ID_LANDING,
    }
    None
}

/// Marks a wire hash as the name of a game constant. Must match the editor.
const CONST_BIT: u64 = 1 << 63;

/// Named constants that blocks can use.
fn named_const(symbol: u64) -> Option<i64> {
    macro_rules! table { ($($name:ident,)*) => {
        $(if symbol == name_hash(stringify!($name)) { return Some(*$name as i64); })*
    }; }
    table! {
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_AIR_ATTACK,
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_AIR_CLIFF,
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_AIR_ESCAPE,
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_AIR_ITEM_THROW,
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_AIR_JUMP_AERIAL,
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_AIR_LANDING,
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_AIR_LASSO,
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_AIR_SPECIAL,
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_AIR_TREAD_JUMP,
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_AIR_WALL_JUMP,
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_GROUND,
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_GROUND_ATTACK,
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_GROUND_CATCH,
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_GROUND_ESCAPE,
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_GROUND_GUARD,
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_GROUND_ITEM,
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_GROUND_JUMP,
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_GROUND_SPECIAL,
        FIGHTER_BAYONETTA_COSTUME_KIND_BAYONETTA_1,
        FIGHTER_BAYONETTA_COSTUME_KIND_DEFAULT,
        FIGHTER_BAYONETTA_COSTUME_KIND_JEANNE,
        MA_MSC_CMD_ATTACK_SET_LERP,
        MA_MSC_CMD_SLOPE_SLOPE,
        MA_MSC_CMD_SLOPE_SLOPE_INTP,
        SLOPE_STATUS_L,
        SLOPE_STATUS_LR,
        SLOPE_STATUS_NONE,
        SLOPE_STATUS_PART,
        SLOPE_STATUS_R,
        SLOPE_STATUS_TERM,
        SLOPE_STATUS_TOP,
        SLOPE_STATUS_TPART,
        WEAPON_EFLAME_ESWORD_MOTION_PART_SET_KIND_OPEM_CLOSE,
        WEAPON_ELIGHT_ESWORD_MOTION_PART_SET_KIND_OPEM_CLOSE,
        ARTICLE_OPE_TARGET_ALL,
        ARTICLE_OPE_TARGET_FIRST,
        ARTICLE_OPE_TARGET_LAST,
        ATTACH_ITEM_GROUP_ALL,
        ATTACH_ITEM_GROUP_BACK,
        ATTACH_ITEM_GROUP_BADGE,
        ATTACH_ITEM_GROUP_BODY,
        ATTACH_ITEM_GROUP_HAT,
        ATTACH_ITEM_GROUP_HAT_ABNORMAL,
        ATTACH_ITEM_GROUP_HIP,
        ATTACH_ITEM_GROUP_NONE,
        ATTACK_DIRECTION_NONE,
        ATTACK_DIRECTION_X,
        ATTACK_DIRECTION_Y,
        ATTACK_DIRECTION_Y_MINUS,
        ATTACK_DIRECTION_Z,
        ATTACK_HEIGHT_HIGH,
        ATTACK_HEIGHT_LOW,
        ATTACK_HEIGHT_MIDDLE,
        ATTACK_LR_CHECK_F,
        ATTACK_LR_CHECK_LEFT,
        ATTACK_LR_CHECK_POS,
        ATTACK_REGION_BODY,
        ATTACK_REGION_NONE,
        ATTACK_REGION_PUNCH,
        ATTACK_REGION_SWORD,
        ATTACK_REGION_THROW,
        ATTACK_SETOFF_KIND_OFF,
        ATTACK_SETOFF_KIND_ON,
        ATTACK_SETOFF_KIND_THRU,
        ATTACK_SOUND_LEVEL_L,
        ATTACK_SOUND_LEVEL_M,
        ATTACK_SOUND_LEVEL_S,
        BATTLE_OBJECT_ID_INVALID,
        CAMERA_QUAKE_KIND_L,
        CAMERA_QUAKE_KIND_M,
        CAMERA_QUAKE_KIND_NONE,
        CAMERA_QUAKE_KIND_S,
        CAMERA_QUAKE_KIND_S_HALF,
        CAMERA_QUAKE_KIND_XL,
        CAMERA_UPDATE_POS_XYZ,
        COLLISION_CATEGORY_MASK_ALL,
        COLLISION_CATEGORY_MASK_FIGHTER,
        COLLISION_CATEGORY_MASK_NO_IF,
        COLLISION_KIND_MASK_ATTACK,
        COLLISION_KIND_REFLECTOR,
        COLLISION_KIND_SHIELD,
        COLLISION_PART_MASK_ALL,
        COLLISION_PART_MASK_HEAD,
        COLLISION_PROPERTY_MASK_REFLECT,
        COLLISION_SITUATION_MASK_A,
        COLLISION_SITUATION_MASK_G,
        COLLISION_SITUATION_MASK_GA,
        COLLISION_SOUND_ATTR_CUTUP,
        COLLISION_SOUND_ATTR_DOLLY_CRITICAL,
        COLLISION_SOUND_ATTR_KICK,
        COLLISION_SOUND_ATTR_NONE,
        COLLISION_SOUND_ATTR_PUNCH,
        COLLISION_TARGET_PROPERTY,
        DAMAGE_NO_REACTION_MODE_ALWAYS,
        DAMAGE_NO_REACTION_MODE_DAMAGE_POWER,
        DAMAGE_NO_REACTION_MODE_DAMAGE_POWER_COUNT,
        DAMAGE_NO_REACTION_MODE_NONE,
        DAMAGE_NO_REACTION_MODE_NORMAL,
        DAMAGE_NO_REACTION_MODE_REACTION_VALUE,
        EFFECT_AXIS_X,
        EFFECT_SCREEN_PRIO_FINAL,
        EFFECT_SCREEN_PRIO_ITEM,
        EFFECT_SCREEN_PRIO_NORMAL,
        EFFECT_SUB_ATTRIBUTE_NO_JOINT_SCALE,
        EF_FLIP_AXIS_XY,
        EF_FLIP_AXIS_YZ,
        EF_FLIP_AXIS_ZX,
        EF_FLIP_NONE,
        EF_FLIP_XY,
        EF_FLIP_YZ,
        EF_FLIP_ZX,
        FIGHTER_ATTACK_ABSOLUTE_KIND_CATCH,
        FIGHTER_ATTACK_ABSOLUTE_KIND_THROW,
        FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_COSTUME_KIND,
        FIGHTER_BAYONETTA_SHOOTING_SLOT_L_ARM,
        FIGHTER_BAYONETTA_SHOOTING_SLOT_L_LEG,
        FIGHTER_BAYONETTA_SHOOTING_SLOT_R_ARM,
        FIGHTER_BAYONETTA_SHOOTING_SLOT_R_LEG,
        FIGHTER_BRAVE_GENERATE_ARTICLE_BLUE,
        FIGHTER_BRAVE_STATUS_SPECIAL_S_WORK_FLOAT_LIGHTNING_OFFSET_X,
        FIGHTER_BUDDY_GENERATE_ARTICLE_PARTNER,
        FIGHTER_CLIFF_HANG_DATA_DEFAULT,
        FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_EFFECT_KIND_SWORD_FLARE,
        FIGHTER_CLOUD_STATUS_FINAL_WORK_FLOAT_ATTACK_TARGET_LR,
        FIGHTER_DIDDY_CLIFF_HANG_DATA_SPECIAL_HI,
        FIGHTER_DIDDY_GENERATE_ARTICLE_BARRELJETS,
        FIGHTER_DIDDY_GENERATE_ARTICLE_GUN,
        FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA,
        FIGHTER_DOLLY_ATTACK_ABSOLUTE_KIND_FINAL,
        FIGHTER_DOLLY_STATUS_SUPER_SPECIAL2_WORK_INT_TARGET_HIT_GROUP,
        FIGHTER_DOLLY_STATUS_SUPER_SPECIAL2_WORK_INT_TARGET_HIT_NO,
        FIGHTER_DOLLY_STATUS_SUPER_SPECIAL2_WORK_INT_TARGET_OBJECT_ID,
        FIGHTER_EDGE_SHIELD_GROUP_KIND_SPECIAL_LW,
        FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD,
        FIGHTER_ELIGHT_INSTANCE_WORK_ID_INT_ESWORD_INHERIT_OPEN_MOTION_KIND,
        FIGHTER_GANON_GENERATE_ARTICLE_SWORD,
        FIGHTER_HAVE_ITEM_WORK_TEMPORARY,
        FIGHTER_IKE_STATUS_SPECIAL_N_WORK_FLOAT_CHARGE_COUNT,
        FIGHTER_IKE_STATUS_SPECIAL_N_WORK_INT_READY_LEVEL,
        FIGHTER_INKLING_INSTANCE_WORK_ID_FLOAT_INK_R,
        FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA,
        FIGHTER_INSTANCE_WORK_ID_FLOAT_FINISH_CAMERA_THROW_RAY_LENGTH,
        FIGHTER_INSTANCE_WORK_ID_INT_COLOR,
        FIGHTER_INSTANCE_WORK_ID_INT_KIND,
        FIGHTER_ITEM_GRASS_PULL_STEP_PICKUP,
        FIGHTER_JACK_CLIFF_HANG_DATA_AIR_LASSO,
        FIGHTER_JACK_INSTANCE_WORK_ID_FLOAT_RESULT_EFFECT_R,
        FIGHTER_JACK_REFLECTOR_KIND_SPECIAL_LW,
        FIGHTER_JACK_SHIELD_GROUP_KIND_SPECIAL_LW,
        FIGHTER_KAMUI_STATUS_SPECIAL_N_WORK_FLOAT_HOLD_RATE,
        FIGHTER_KINETIC_ENERGY_ID_CONTROL,
        FIGHTER_KINETIC_ENERGY_ID_GRAVITY,
        FIGHTER_KINETIC_TYPE_FALL,
        FIGHTER_KINETIC_TYPE_PASSIVE_CEIL,
        FIGHTER_KIRBY_GENERATE_ARTICLE_HAMMER,
        FIGHTER_KIRBY_GENERATE_ARTICLE_STONE,
        FIGHTER_KIRBY_INSTANCE_WORK_ID_FLOAT_INKLING_SPECIAL_N_INK_B,
        FIGHTER_KIRBY_INSTANCE_WORK_ID_FLOAT_INKLING_SPECIAL_N_INK_G,
        FIGHTER_KIRBY_INSTANCE_WORK_ID_FLOAT_INKLING_SPECIAL_N_INK_R,
        FIGHTER_KOOPAJR_GENERATE_ARTICLE_HAMMER,
        FIGHTER_LITTLEMAC_STATUS_WORK_ID_FLOAT_SPECIAL_N_CHARGE_RATE,
        FIGHTER_LUCARIO_INSTANCE_WORK_ID_FLOAT_ATTACK_AIR_LW_SPEED,
        FIGHTER_LUCARIO_INSTANCE_WORK_ID_FLOAT_CURR_AURAPOWER,
        FIGHTER_LUCARIO_POWER_PUNCH_STATUS_WORK_ID_FLOAT_HAKKEI_NEAR_SCALE,
        FIGHTER_LUCARIO_SHIELD_GROUP_KIND_SPLIT,
        FIGHTER_LUCARIO_SHIELD_KIND_SPLIT,
        FIGHTER_LUCARIO_SPECIAL_N_STATUS_WORK_ID_FLOAT_CHARGE_RATE,
        FIGHTER_LUCAS_CLIFF_HANG_DATA_SPECIAL_HI,
        FIGHTER_LUCAS_CLIFF_HANG_DATA_START,
        FIGHTER_LUIGI_STATUS_SPECIAL_S_CHARGE_WORK_FLOAT_CHARGE,
        FIGHTER_MARIOD_REFLECTOR_KIND_DRMANTLE,
        FIGHTER_MARIO_REFLECTOR_KIND_MANTLE,
        FIGHTER_MASTER_CLIFF_HANG_DATA_AIR_LASSO,
        FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1,
        FIGHTER_MASTER_GENERATE_ARTICLE_SWORD,
        FIGHTER_MASTER_STATUS_SPECIAL_LW_INT_VOICE_TYPE,
        FIGHTER_METAKNIGHT_GENERATE_ARTICLE_MANTLE,
        FIGHTER_METAKNIGHT_STATUS_SPECIAL_N_SPIN_WORK_INT_EFFECT_HANDLE,
        FIGHTER_MEWTWO_CLIFF_HANG_DATA_SPECIAL_HI,
        FIGHTER_MIIFIGHTER_INSTANCE_WORK_ID_FLOAT_NENSYO_KICK_CHARGE_RATE,
        FIGHTER_MIISWORDSMAN_REFLECTOR_KIND_REVERSE_SLASH,
        FIGHTER_MIISWORDSMAN_STATUS_FINAL_WORK_ID_FLOAT_WAVE_ANGLE,
        FIGHTER_MIISWORDSMAN_STATUS_SHIPPU_SLASH_WORK_FLOAT_CHARGE_RATE,
        FIGHTER_MOTION_PART_SET_KIND_UPPER_BODY,
        FIGHTER_NESS_CLIFF_HANG_DATA_DEFAULT,
        FIGHTER_NESS_CLIFF_HANG_DATA_SPECIAL_HI,
        FIGHTER_PACKUN_STATUS_SPECIAL_LW_WORK_FLOAT_CHARGE_RATE,
        FIGHTER_PACKUN_STATUS_SPECIAL_N_WORK_FLOAT_SHOOT_START_SPIKEBALL_DISTANCE,
        FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL,
        FIGHTER_PALUTENA_REFLECTOR_KIND_REFLECTOR,
        FIGHTER_PEACH_SHIELD_GROUP_KIND_KINOPIO_GUARD,
        FIGHTER_PEACH_SHIELD_KIND_KINOPIO_GUARD,
        FIGHTER_PEACH_STATUS_SPECIAL_LW_WORK_INT_IS_DAIKON_GENERATABLE,
        FIGHTER_PIKMIN_STATUS_ATTACK_AIR_WORK_INT_HAVE_PIKMIN,
        FIGHTER_PIKMIN_STATUS_PULL_OUT_WORK_FLOAT_MOT_RATE,
        FIGHTER_PIT_REFLECTOR_GROUP_SPECIAL_LW,
        FIGHTER_PIT_REFLECTOR_GROUP_SPECIAL_S,
        FIGHTER_PIT_REFLECTOR_KIND_SPECIAL_S,
        FIGHTER_PIT_SHIELD_GROUP_KIND_SPECIAL_LW,
        FIGHTER_PURIN_STATUS_SPECIAL_N_WORK_FLOAT_SPEED,
        FIGHTER_REFLECTOR_GROUP_EXTEND,
        FIGHTER_REFLECTOR_GROUP_HOMERUNBAT,
        FIGHTER_REFLET_INSTANCE_WORK_ID_FLOAT_SPECIAL_S_CURRENT_POINT,
        FIGHTER_REFLET_INSTANCE_WORK_ID_INT_CATCH_EFFECT_HANDLE,
        FIGHTER_RIDLEY_INSTANCE_WORK_ID_INT_DISABLE_SPECIAL_LW_FINISH_COUNT,
        FIGHTER_ROBOT_GENERATE_ARTICLE_GYRO,
        FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BEAM_ENERGY_VALUE,
        FIGHTER_ROBOT_REFLECTOR_KIND_ARMSPIN,
        FIGHTER_ROBOT_STATUS_BEAM_WORK_FLOAT_ENERGY_VALUE,
        FIGHTER_ROCKMAN_ARMFORM_HAND,
        FIGHTER_ROCKMAN_ARMFORM_ROCKBUSTER,
        FIGHTER_ROCKMAN_ARMFORM_WASTEHEAT,
        FIGHTER_ROCKMAN_ARM_LEFT,
        FIGHTER_ROCKMAN_ARM_RIGHT,
        FIGHTER_ROY_STATUS_SPECIAL_N_WORK_INT_READY_LEVEL,
        FIGHTER_RYU_INSTANCE_WORK_ID_FLAG_FINAL_HIT_CANCEL,
        FIGHTER_RYU_SAVING_LV_1,
        FIGHTER_RYU_SAVING_LV_2,
        FIGHTER_RYU_SAVING_LV_3,
        FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO,
        FIGHTER_STATUS_ATTACK_FLAG_SMASH_SMASH_HOLD_TO_ATTACK,
        FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD,
        FIGHTER_STATUS_ATTACK_WORK_FLOAT_SMASH_RESTART_FRAME,
        FIGHTER_STATUS_ITEM_GRASS_PULL_WORK_INT_NEXT_STEP,
        FIGHTER_STATUS_ITEM_SHOOT_WORK_FLAG_SSCOPE_AERIAL_REQ_EFFECT,
        FIGHTER_STATUS_ITEM_SHOOT_WORK_INT_SSCOPE_CHARGE,
        FIGHTER_STATUS_ITEM_SWING_WORK_INT_S4_HOLD_FRAME,
        FIGHTER_STATUS_JUMP_FLAG_GIMMICK_SPRING_JUMP_FROM_RING,
        FIGHTER_STATUS_KIND_ATTACK,
        FIGHTER_STATUS_KIND_SWALLOWED,
        FIGHTER_STATUS_KIND_WAIT,
        FIGHTER_STATUS_SQUAT_FLAG_REQUEST_SQUAT_SE,
        FIGHTER_STATUS_THROW_WORK_INT_TARGET_HIT_GROUP,
        FIGHTER_STATUS_THROW_WORK_INT_TARGET_HIT_NO,
        FIGHTER_STATUS_THROW_WORK_INT_TARGET_OBJECT,
        FIGHTER_STATUS_TRANSITION_GROUP_CHK_AIR_LANDING,
        FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_HI,
        FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN,
        FIGHTER_STATUS_WORK_ID_FLOAT_RESERVE_HOLD_RATE,
        FIGHTER_SZEROSUIT_STATUS_SPECIAL_N_WORK_FLOAT_CHARGE,
        FIGHTER_TANTAN_CLIFF_HANG_DATA_AIR_LASSO,
        GROUND_CLIFF_CHECK_KIND_ALWAYS,
        GROUND_CLIFF_CHECK_KIND_ALWAYS_BOTH_SIDES,
        GROUND_CLIFF_CHECK_KIND_ALWAYS_BOTH_SIDES_NEAR,
        GROUND_CLIFF_CHECK_KIND_NONE,
        GROUND_CLIFF_CHECK_KIND_ON_DROP,
        GROUND_CLIFF_CHECK_KIND_ON_DROP_BOTH_SIDES,
        GROUND_CORRECT_KIND_GROUND,
        HIT_STATUS_INVINCIBLE,
        HIT_STATUS_MASK_ALL,
        HIT_STATUS_MASK_NORMAL,
        HIT_STATUS_NORMAL,
        HIT_STATUS_OFF,
        HIT_STATUS_XLU,
        ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_ANGLE,
        ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_POWER,
        ITEM_FIGHTER_VAR_FLOAT_ITEM_THROW_SPEED,
        ITEM_SIZE_LIGHT,
        ITEM_SWING_ACTION_BEGIN,
        KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN,
        MA_MSC_CMD_ATTACK_NODE,
        MA_MSC_CMD_ATTACK_SET_LERP,
        MA_MSC_CMD_CAMERA_CAM_OFFSET,
        MA_MSC_CMD_CAMERA_CAM_RECT,
        MA_MSC_CMD_CAMERA_CAM_RESET,
        MA_MSC_CMD_CAMERA_SET_UPDATE_POS,
        MA_MSC_CMD_EFFECT_AFTER_IMAGE3_ON,
        MA_MSC_CMD_GRAB_CLEAR,
        MA_MSC_CMD_GRAB_CLEAR_ALL,
        MA_MSC_CMD_PHYSICS_PH_SWING_STOP,
        MA_MSC_CMD_PHYSICS_START_CHARGE,
        MA_MSC_CMD_REFLECTOR,
        MA_MSC_CMD_SEARCH_SEARCH_SCH_CLR,
        MA_MSC_CMD_SEARCH_SEARCH_SCH_CLR_ALL,
        MA_MSC_CMD_SEARCH_SET_OPPONENT,
        MA_MSC_CMD_SHIELD_OFF,
        MA_MSC_CMD_SHIELD_ON,
        MA_MSC_CMD_SLOPE_SLOPE,
        MA_MSC_CMD_SLOPE_SLOPE_INTP,
        MA_MSC_CMD_SOUND_IS_PLAYING_STATUS,
        MA_MSC_CMD_SOUND_STOP_SE_STATUS,
        MA_MSC_DAMAGE_DAMAGE_NO_REACTION,
        SITUATION_KIND_GROUND,
        SLOPE_STATUS_L,
        SLOPE_STATUS_LR,
        SLOPE_STATUS_NONE,
        SLOPE_STATUS_PART,
        SLOPE_STATUS_R,
        SLOPE_STATUS_TOP,
        SLOPE_STATUS_TPART,
        TRAIL_BLEND_BLEND_SRC_ONE,
        WEAPON_BAYONETTA_BAT_INSTANCE_WORK_ID_FLOAT_MOTION_RATE,
        WEAPON_EDGE_FLASH_INSTANCE_WORK_ID_FLOAT_ATTACK_RATIO,
        WEAPON_EFLAME_ESWORD_MOTION_PART_SET_KIND_OPEM_CLOSE,
        WEAPON_GEKKOUGA_SHURIKEN_INSTANCE_WORK_ID_FLOAT_CHARGE_LEVEL_RATIO,
        WEAPON_INKLING_BLASTER_INSTANCE_WORK_ID_FLOAT_R,
        WEAPON_INKLING_BRUSH_INSTANCE_WORK_ID_FLOAT_R,
        WEAPON_INKLING_INKBULLET_INSTANCE_WORK_ID_FLOAT_R,
        WEAPON_INKLING_MEGAPHONELASER_INSTANCE_WORK_ID_FLOAT_R,
        WEAPON_INKLING_ROLLER_INSTANCE_WORK_ID_FLOAT_R,
        WEAPON_INKLING_SLOSHER_INSTANCE_WORK_ID_FLOAT_R,
        WEAPON_INKLING_SPLASHBOMB_INSTANCE_WORK_ID_FLOAT_R,
        WEAPON_INKLING_SPLASH_INSTANCE_WORK_ID_FLOAT_R,
        WEAPON_INKLING_SQUID_INSTANCE_WORK_ID_FLOAT_R,
        WEAPON_KAMUI_DRAGONHAND_INSTANCE_WORK_ID_FLOAT_HOLD_RATE,
        WEAPON_KAMUI_RYUSENSYA_INSTANCE_WORK_ID_FLOAT_HOLD_RATE,
        WEAPON_KOOPAJR_CANNONBALL_INSTANCE_WORK_ID_FLOAT_CHARGE,
        WEAPON_LUCARIO_AURABALL_INSTANCE_WORK_ID_FLOAT_AURAPOWER,
        WEAPON_LUCARIO_AURABALL_INSTANCE_WORK_ID_FLOAT_CHARGE_RATE,
        WEAPON_LUCARIO_AURABALL_INSTANCE_WORK_ID_FLOAT_SE_AURAPOWER_MIDDLE,
        WEAPON_LUCAS_PK_FREEZE_INSTANCE_WORK_ID_FLOAT_COUNT,
        WEAPON_MASTER_SWORD_STATUS_KIND_EXTEND,
        WEAPON_MEWTWO_SHADOWBALL_INSTANCE_WORK_ID_FLOAT_CHARGE_RATE,
        WEAPON_MIIGUNNER_GUNNERCHARGE_INSTANCE_WORK_ID_FLOAT_CHARGE,
        WEAPON_MIIGUNNER_STEALTHBOMB_S_INSTANCE_WORK_ID_FLOAT_RATE,
        WEAPON_NESS_PK_FLASH_INSTANCE_WORK_ID_FLOAT_COUNT,
        WEAPON_PIKMIN_PIKMIN_STATUS_PULL_OUT_START_WORK_FLOAT_MOT_RATE,
        WEAPON_RIDLEY_BREATH_INSTANCE_WORK_ID_FLOAT_CHARGE_RATE,
        WEAPON_ROSETTA_TICO_STATUS_SPECIAL_N_COMMON_WORK_FLOAT_RATE,
        WEAPON_SAMUS_CSHOT_INSTANCE_WORK_ID_FLOAT_CHARGE,
        WEAPON_WIIFIT_SUNBULLET_INSTANCE_WORK_ID_FLOAT_CHARGE_LEVEL_RATIO,
        WEAPON_ZELDA_DEIN_S_INSTANCE_WORK_ID_FLOAT_RATE,
    }
    None
}

/// A constant by name: a shared one, or one of this fighter's own.
unsafe fn resolve(boma: *mut Boma, symbol: u64) -> Option<i64> {
    use crate::slight::gimmicks::registry::IdentifierKind as Id;
    named_const(symbol)
        .or_else(|| {
            [
                Id::WorkInt,
                Id::WorkFloat,
                Id::WorkFlag,
                Id::FighterStatus,
                Id::WeaponStatus,
                Id::Article,
                Id::Item,
                Id::TransitionTerm,
            ]
            .into_iter()
            .find_map(|kind| find_id(boma, symbol, kind).map(i64::from))
        })
        .or_else(|| {
            crate::slight::diag::note(format!("node step: unknown game value {symbol:#x}"));
            None
        })
}

unsafe fn push(lua_state: u64, boma: *mut Boma, args: &[LuaArg]) {
    let mut agent = smash::lib::L2CAgent::new(lua_state);
    for arg in args {
        let mut value = match arg {
            LuaArg::Hash(hash) if hash & CONST_BIT != 0 => {
                LuaArg::Int(resolve(boma, hash & !CONST_BIT).unwrap_or(0)).to_l2c()
            }
            other => other.to_l2c(),
        };
        agent.push_lua_stack(&mut value);
    }
}

/// Calls the editor sends by name, with the arguments its block holds.
unsafe fn generic(lua_state: u64, boma: *mut Boma, name: &str, args: &[LuaArg]) -> Option<bool> {
    use smash::app::lua_bind::{
        FighterAreaModuleImpl as Area, GrabModule, GroundModule, ModelModule,
    };
    use smash::app::sv_animcmd as cmd;
    use smash::app::ArticleOperationTarget as Target;
    use smash::phx::Hash40;
    let num = |i: usize| match args.get(i) {
        Some(LuaArg::Num(value)) => *value,
        Some(LuaArg::Int(value)) => *value as f32,
        _ => 0.0,
    };
    let int = |i: usize| match args.get(i) {
        Some(LuaArg::Int(value)) => *value,
        Some(LuaArg::Num(value)) => *value as i64,
        Some(LuaArg::Hash(hash)) if hash & CONST_BIT != 0 => {
            resolve(boma, hash & !CONST_BIT).unwrap_or(0)
        }
        Some(LuaArg::Hash(hash)) => *hash as i64,
        _ => 0,
    };
    let flag = |i: usize| matches!(args.get(i), Some(LuaArg::Bool(true)));
    let hash = |i: usize| match args.get(i) {
        Some(LuaArg::Hash(hash)) => Hash40::new_raw(*hash & !CONST_BIT),
        _ => Hash40::new_raw(0),
    };
    let all = Target(*ARTICLE_OPE_TARGET_ALL);
    match name {
        "AttackModule::set_size" => AttackModule::set_size(boma, int(0) as i32, num(1)),
        "AttackModule::set_add_reaction_frame" => {
            AttackModule::set_add_reaction_frame(boma, int(0) as i32, num(1), flag(2))
        }
        "AttackModule::set_add_reaction_frame_revised" => {
            AttackModule::set_add_reaction_frame_revised(boma, int(0) as i32, num(1), flag(2))
        }
        "AttackModule::set_damage_shake_scale" => {
            AttackModule::set_damage_shake_scale(boma, num(0))
        }
        "AttackModule::set_no_uniq_effect_all" => {
            AttackModule::set_no_uniq_effect_all(boma, flag(0), flag(1))
        }
        "AttackModule::init_attack_pos" => {
            AttackModule::init_attack_pos(boma, int(0) as i32);
        }
        "AttackModule::set_no_damage_fly_smoke_all" => {
            AttackModule::set_no_damage_fly_smoke_all(boma, flag(0), flag(1))
        }
        "AttackModule::disable_tip" => {
            AttackModule::disable_tip(boma);
        }
        "AttackModule::enable_safe_pos" => {
            AttackModule::enable_safe_pos(boma);
        }
        "AttackModule::set_attack_keep_rumble" => {
            AttackModule::set_attack_keep_rumble(boma, int(0) as i32, flag(1))
        }
        "AttackModule::set_force_reaction" => {
            AttackModule::set_force_reaction(boma, int(0) as i32, flag(1), flag(2))
        }
        "AttackModule::set_attack_level" => {
            AttackModule::set_attack_level(boma, int(0) as i32, int(1) as u8)
        }
        "AttackModule::set_invalid_xlu" => {
            AttackModule::set_invalid_xlu(boma, int(0) as i32, flag(1))
        }
        "AttackModule::set_no_finish_camera_ex" => {
            AttackModule::set_no_finish_camera_ex(boma, int(0) as i32, flag(1), flag(2))
        }
        "AttackModule::set_final_finish_cut_in" => AttackModule::set_final_finish_cut_in(
            boma,
            int(0) as i32,
            flag(1),
            flag(2),
            num(3),
            flag(4),
        ),
        "AttackModule::set_attack_camera_quake_forced" => {
            AttackModule::set_attack_camera_quake_forced(
                boma,
                int(0) as i32,
                int(1) as i32,
                flag(2),
            )
        }
        "AttackModule::set_ink_value" => AttackModule::set_ink_value(boma, int(0) as i32, num(1)),
        "AttackModule::set_target_category" => {
            AttackModule::set_target_category(boma, int(0) as i32, int(1) as u32)
        }
        "AttackModule::set_attack_height_all" => AttackModule::set_attack_height_all(
            boma,
            smash::app::AttackHeight(int(0) as i32),
            flag(1),
        ),
        "ItemModule::set_have_item_visibility" => {
            ItemModule::set_have_item_visibility(boma, flag(0), int(1) as i32)
        }
        "ItemModule::set_attach_item_visibility" => {
            ItemModule::set_attach_item_visibility(boma, flag(0), int(1) as u8)
        }
        "FighterAreaModuleImpl::enable_fix_jostle_area" => {
            Area::enable_fix_jostle_area(boma, num(0), num(1));
        }
        "FighterAreaModuleImpl::enable_fix_jostle_area_xy" => {
            Area::enable_fix_jostle_area_xy(boma, num(0), num(1), num(2), num(3));
        }
        "VisibilityModule::set_int64" => VisibilityModule::set_int64(boma, int(0), int(1)),
        "VisibilityModule::set_default_int64" => VisibilityModule::set_default_int64(boma, int(0)),
        "VisibilityModule::set_status_default_int64" => {
            VisibilityModule::set_status_default_int64(boma, int(0), int(1))
        }
        "ArticleModule::change_status" => {
            ArticleModule::change_status(boma, int(0) as i32, int(1) as i32, all);
        }
        "ArticleModule::change_status_exist" => {
            ArticleModule::change_status_exist(boma, int(0) as i32, int(1) as i32);
        }
        "ArticleModule::change_motion" => {
            ArticleModule::change_motion(boma, int(0) as i32, hash(1), flag(2), num(3));
        }
        "ArticleModule::shoot_exist" => {
            ArticleModule::shoot_exist(boma, int(0) as i32, all, false);
        }
        "ArticleModule::set_visibility" => {
            ArticleModule::set_visibility(boma, int(0) as i32, hash(1), hash(2), all)
        }
        "GrabModule::set_rebound" => GrabModule::set_rebound(boma, flag(0)),
        "GrabModule::set_constraint" => GrabModule::set_constraint(boma, int(0) as i32, flag(1)),
        "WorkModule::set_int" => WorkModule::set_int(boma, int(0) as i32, int(1) as i32),
        "WorkModule::set_float" => WorkModule::set_float(boma, num(0), int(1) as i32),
        "ModelModule::disable_gold_eye" => {
            ModelModule::disable_gold_eye(boma);
        }
        "GroundModule::select_cliff_hangdata" => {
            GroundModule::select_cliff_hangdata(boma, int(0) as u32);
        }
        "StatusModule::change_status_request_from_script" => {
            StatusModule::change_status_request_from_script(boma, int(0) as i32, flag(1));
        }
        "ControlModule::set_rumble" => {
            ControlModule::set_rumble(boma, hash(0), int(1) as i32, flag(2), int(3) as u32)
        }
        "macros::FT_ADD_DAMAGE" => {
            push(lua_state, boma, args);
            cmd::FT_ADD_DAMAGE(lua_state);
        }
        "macros::FT_LEAVE_NEAR_OTTOTTO" => {
            push(lua_state, boma, args);
            cmd::FT_LEAVE_NEAR_OTTOTTO(lua_state);
        }
        "macros::COL_PRI" => {
            push(lua_state, boma, args);
            cmd::COL_PRI(lua_state);
        }
        "macros::QUAKE" => {
            push(lua_state, boma, args);
            cmd::QUAKE(lua_state);
        }
        "macros::SLOW_OPPONENT" => {
            push(lua_state, boma, args);
            cmd::SLOW_OPPONENT(lua_state);
        }
        "macros::FILL_SCREEN_MODEL_COLOR" => {
            push(lua_state, boma, args);
            cmd::FILL_SCREEN_MODEL_COLOR(lua_state);
        }
        "macros::CANCEL_FILL_SCREEN" => {
            push(lua_state, boma, args);
            cmd::CANCEL_FILL_SCREEN(lua_state);
        }
        "macros::RUMBLE_HIT_STATUS" => {
            push(lua_state, boma, args);
            cmd::RUMBLE_HIT_STATUS(lua_state);
        }
        "macros::SHOOT_ITEM_BULLET" => {
            cmd::SHOOT_ITEM_BULLET(lua_state);
        }
        "macros::ATK_SET_SHIELD_SETOFF_MUL" => {
            push(lua_state, boma, args);
            cmd::ATK_SET_SHIELD_SETOFF_MUL(lua_state);
        }
        "macros::ATK_SET_SHIELD_SETOFF_MUL2" => {
            push(lua_state, boma, args);
            cmd::ATK_SET_SHIELD_SETOFF_MUL2(lua_state);
        }
        "macros::ATK_SET_SHIELD_SETOFF_MUL_arg3" => {
            push(lua_state, boma, args);
            cmd::ATK_SET_SHIELD_SETOFF_MUL_arg3(lua_state);
        }
        "macros::ATK_SET_SHIELD_SETOFF_MUL_arg4" => {
            push(lua_state, boma, args);
            cmd::ATK_SET_SHIELD_SETOFF_MUL_arg4(lua_state);
        }
        "macros::ATK_SET_SHIELD_SETOFF_MUL_arg5" => {
            push(lua_state, boma, args);
            cmd::ATK_SET_SHIELD_SETOFF_MUL_arg5(lua_state);
        }
        "macros::CHECK_FINISH_CAMERA" => {
            push(lua_state, boma, args);
            cmd::CHECK_FINISH_CAMERA(lua_state);
        }
        "macros::CHECK_VALID_FINAL_START_CAMERA" => {
            push(lua_state, boma, args);
            cmd::CHECK_VALID_FINAL_START_CAMERA(lua_state);
        }
        "macros::FT_SET_FINAL_FEAR_FACE" => {
            push(lua_state, boma, args);
            cmd::FT_SET_FINAL_FEAR_FACE(lua_state);
        }
        "attack!" => {
            push(lua_state, boma, args);
            smash::app::sv_module_access::attack(lua_state);
        }
        "slope!" => {
            push(lua_state, boma, args);
            smash::app::sv_module_access::slope(lua_state);
        }
        "AttackModule::set_no_dead_all" => AttackModule::set_no_dead_all(boma, flag(0), flag(1)),
        "AttackModule::set_catch_only_all" => {
            AttackModule::set_catch_only_all(boma, flag(0), flag(1))
        }
        "AttackModule::set_no_finish_camera" => {
            AttackModule::set_no_finish_camera(boma, int(0) as i32, flag(1), flag(2))
        }
        "AttackModule::set_attack_reference_joint_id" => {
            use smash::app::AttackDirectionAxis as Axis;
            AttackModule::set_attack_reference_joint_id(
                boma,
                hash(0),
                Axis(int(1) as i32),
                Axis(int(2) as i32),
                Axis(int(3) as i32),
            )
        }
        "AttackModule::set_vec_target_pos" => {
            let target = smash::phx::Vector2f {
                x: num(2),
                y: num(3),
            };
            AttackModule::set_vec_target_pos(
                boma,
                int(0) as i32,
                hash(1),
                &target,
                int(4) as u32,
                flag(5),
            )
        }
        "AttackModule::set_optional_hit_effect" => {
            AttackModule::set_optional_hit_effect(boma, int(0) as i32, hash(1))
        }
        "AttackModule::set_optional_hit_sound" => {
            AttackModule::set_optional_hit_sound(boma, int(0) as i32, hash(1))
        }
        "EffectModule::request_post_effect_line_circle" => {
            use smash::phx::simd::{Vector2, Vector3};
            smash::app::lua_bind::EffectModule::request_post_effect_line_circle(
                boma,
                hash(0),
                hash(1),
                Vector2 {
                    vec: [num(2), num(3)],
                },
                Vector3 {
                    vec: [num(4), num(5), num(6)],
                },
                flag(7),
                num(8),
                num(9),
            );
        }
        "EffectModule::remove_post_effect_line" => {
            smash::app::lua_bind::EffectModule::remove_post_effect_line(
                boma,
                int(0) as i32,
                flag(1),
            );
        }
        "WorkModule::set_int64" => WorkModule::set_int64(boma, int(0), int(1) as i32),
        "WorkModule::enable_transition_term_group" => {
            WorkModule::enable_transition_term_group(boma, int(0) as i32)
        }
        "WorkModule::unable_transition_term_group_ex" => {
            WorkModule::unable_transition_term_group_ex(boma, int(0) as i32)
        }
        "ArticleModule::add_motion_partial" => {
            // An unknown part set must not fall back to part set 0.
            if !matches!(args.get(1), Some(LuaArg::Hash(h)) if resolve(boma, h & !CONST_BIT).is_some())
            {
                return Some(false);
            }
            ArticleModule::add_motion_partial(
                boma,
                int(0) as i32,
                int(1) as i32,
                hash(2),
                num(3),
                num(4),
                flag(5),
                flag(6),
                num(7),
                flag(8),
                flag(9),
                flag(10),
            );
        }
        _ => return None,
    }
    Some(true)
}
