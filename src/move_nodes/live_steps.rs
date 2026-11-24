//! Sends a move's added blocks to the running match.
//!
//! The older rule families retune what the game's own script does. A move made from blocks also
//! needs things the script never did, so this walks the blocks the way they run, compares them to
//! the game's script, and turns the differences into steps the plugin performs itself. A block
//! inside an If carries its condition along and the plugin checks it in the match.
use super::model::{
    MoveCondition, MoveGraph, MoveNode, NativeComparison, NativeIntOperand, NodeKind, Track,
};
use crate::data::{ArticleLifecycleCall, ExcuteStmt};
use crate::game_link::{HitboxRuleWire, InjectRuleWire, LiveGuard, LuaArgWire as A, CAT_NODE_STEP};
use crate::gimmicks::registry::{self, IdentifierKind};

/// One block's effect at one frame.
#[derive(Clone, PartialEq)]
struct Flat {
    frame: u32,
    guard: Option<LiveGuard>,
    step: Option<(String, Vec<A>)>,
}

fn article_id(fighter: &str, symbol: &str) -> Option<i64> {
    registry::resolve(
        fighter,
        symbol.trim_start_matches('*'),
        IdentifierKind::Article,
    )
    .ok()
    .map(|entry| entry.value as i64)
}

/// The plugin step for a command. `None` means an older family carries it, or nothing can yet.
fn step_for(fighter: &str, command: &ExcuteStmt) -> Option<(String, Vec<A>)> {
    let (name, args): (&str, Vec<A>) = match command {
        ExcuteStmt::SetSpeed(call) => (
            "SET_SPEED",
            vec![A::Num(call.speed_x), A::Num(call.speed_y)],
        ),
        ExcuteStmt::AddSpeedNoLimit(call) => (
            "ADD_SPEED_NO_LIMIT",
            vec![A::Num(call.speed_x), A::Num(call.speed_y)],
        ),
        ExcuteStmt::SetAir => ("SET_AIR", Vec::new()),
        ExcuteStmt::ReverseLr => ("REVERSE_LR", Vec::new()),
        ExcuteStmt::KineticClearSpeedAll => ("clear_speed_all", Vec::new()),
        ExcuteStmt::VisibilitySetWhole { visible } => ("set_whole", vec![A::Bool(*visible)]),
        ExcuteStmt::ArticleLifecycle(call) => match call {
            ArticleLifecycleCall::Generate { symbol }
            | ArticleLifecycleCall::GenerateEnabled { symbol } => (
                "generate_article",
                vec![A::Int(article_id(fighter, symbol)?)],
            ),
            ArticleLifecycleCall::Shoot { symbol, .. }
            | ArticleLifecycleCall::ShootExist { symbol, .. } => {
                ("shoot_article", vec![A::Int(article_id(fighter, symbol)?)])
            }
            ArticleLifecycleCall::RemoveAll { symbol } => {
                ("remove_article", vec![A::Int(article_id(fighter, symbol)?)])
            }
            ArticleLifecycleCall::SetVisibilityWhole { symbol, visible } => (
                "article_visibility",
                vec![A::Int(article_id(fighter, symbol)?), A::Bool(*visible)],
            ),
            _ => return generic_step(command),
        },
        ExcuteStmt::HitModuleSetStatusAllNormal => ("hit_all_normal", Vec::new()),
        ExcuteStmt::StartCutIn => ("start_cutin", Vec::new()),
        ExcuteStmt::StartInfoFlashEye => ("flash_eye", Vec::new()),
        ExcuteStmt::JostleSetStatus { enabled } => ("jostle", vec![A::Bool(*enabled)]),
        ExcuteStmt::AttackAbsApplyThrow => ("apply_throw", Vec::new()),
        ExcuteStmt::CameraZoomOut => ("camera_zoom_out", Vec::new()),
        ExcuteStmt::SlowWhole(crate::data::SlowWholeCall::Set { multiplier, frames }) => (
            "slow_whole",
            vec![A::Int(i64::from(*multiplier)), A::Int(i64::from(*frames))],
        ),
        ExcuteStmt::SlowWhole(crate::data::SlowWholeCall::Clear) => ("slow_clear", Vec::new()),
        ExcuteStmt::WorkTransitionTerm(call) => {
            let on = matches!(call.action, crate::data::WorkTransitionTermAction::Enable);
            let off = matches!(call.action, crate::data::WorkTransitionTermAction::Unable);
            if !on && !off {
                return generic_step(command);
            }
            let term = call.transition_term.trim_start_matches('*');
            (
                if on { "enable_term" } else { "unable_term" },
                vec![A::Hash(hash40::hash40(term).0)],
            )
        }
        // The plugin finds the receiver by name; there is no Lua string type on the wire.
        ExcuteStmt::GimmickEvent { name } => {
            return Some((format!("event:main:{name}"), Vec::new()))
        }
        ExcuteStmt::GimmickChartEvent { chart_id, name } => {
            return Some((format!("event:{chart_id}:{name}"), Vec::new()))
        }
        // Field names ride in the command; the values follow in the same order.
        ExcuteStmt::GimmickChartEventValues {
            chart_id,
            name,
            arguments,
        } => {
            use crate::gimmicks::behavior::ConstValue;
            let fields: Vec<&str> = arguments.iter().map(|arg| arg.field.as_str()).collect();
            let values = arguments
                .iter()
                .map(|arg| match arg.value {
                    ConstValue::Int(value) => A::Int(i64::from(value)),
                    ConstValue::Float(value) => A::Num(value),
                    ConstValue::Flag(value) => A::Bool(value),
                })
                .collect();
            return Some((
                format!("eventv:{chart_id}:{name}:{}", fields.join(",")),
                values,
            ));
        }
        ExcuteStmt::WorkFlag(call) => {
            let symbol = call.flag.trim_start_matches('*').to_owned();
            let on = matches!(call.action, crate::data::WorkFlagAction::On);
            (
                if on { "on_flag" } else { "off_flag" },
                vec![A::Hash(hash40::hash40(&symbol).0)],
            )
        }
        _ => return generic_step(command),
    };
    Some((name.to_owned(), args))
}

/// Game calls the plugin can make by name. **Must match the plugin's `generic` table.**
const GENERIC: &[&str] = &[
    "AttackModule::set_size",
    "AttackModule::set_add_reaction_frame",
    "AttackModule::set_add_reaction_frame_revised",
    "AttackModule::set_damage_shake_scale",
    "AttackModule::set_no_uniq_effect_all",
    "AttackModule::init_attack_pos",
    "AttackModule::set_no_damage_fly_smoke_all",
    "AttackModule::disable_tip",
    "AttackModule::enable_safe_pos",
    "AttackModule::set_attack_keep_rumble",
    "AttackModule::set_force_reaction",
    "AttackModule::set_attack_level",
    "AttackModule::set_invalid_xlu",
    "AttackModule::set_no_finish_camera_ex",
    "AttackModule::set_final_finish_cut_in",
    "AttackModule::set_attack_camera_quake_forced",
    "AttackModule::set_ink_value",
    "AttackModule::set_target_category",
    "AttackModule::set_attack_height_all",
    "ItemModule::set_have_item_visibility",
    "ItemModule::set_attach_item_visibility",
    "FighterAreaModuleImpl::enable_fix_jostle_area",
    "FighterAreaModuleImpl::enable_fix_jostle_area_xy",
    "VisibilityModule::set_int64",
    "VisibilityModule::set_default_int64",
    "VisibilityModule::set_status_default_int64",
    "ArticleModule::change_status",
    "ArticleModule::change_status_exist",
    "ArticleModule::change_motion",
    "ArticleModule::shoot_exist",
    "ArticleModule::set_visibility",
    "GrabModule::set_rebound",
    "GrabModule::set_constraint",
    "WorkModule::set_int",
    "WorkModule::set_float",
    "ModelModule::disable_gold_eye",
    "GroundModule::select_cliff_hangdata",
    "StatusModule::change_status_request_from_script",
    "ControlModule::set_rumble",
    "macros::FT_ADD_DAMAGE",
    "macros::FT_LEAVE_NEAR_OTTOTTO",
    "macros::COL_PRI",
    "macros::QUAKE",
    "macros::SLOW_OPPONENT",
    "macros::FILL_SCREEN_MODEL_COLOR",
    "macros::CANCEL_FILL_SCREEN",
    "macros::RUMBLE_HIT_STATUS",
    "macros::SHOOT_ITEM_BULLET",
    "macros::ATK_SET_SHIELD_SETOFF_MUL",
    "macros::ATK_SET_SHIELD_SETOFF_MUL2",
    "macros::ATK_SET_SHIELD_SETOFF_MUL_arg3",
    "macros::ATK_SET_SHIELD_SETOFF_MUL_arg4",
    "macros::ATK_SET_SHIELD_SETOFF_MUL_arg5",
    "macros::CHECK_FINISH_CAMERA",
    "macros::CHECK_VALID_FINAL_START_CAMERA",
    "macros::FT_SET_FINAL_FEAR_FACE",
    "attack!",
    "slope!",
    "AttackModule::set_no_dead_all",
    "AttackModule::set_catch_only_all",
    "AttackModule::set_no_finish_camera",
    "AttackModule::set_attack_reference_joint_id",
    "AttackModule::set_vec_target_pos",
    "AttackModule::set_optional_hit_effect",
    "AttackModule::set_optional_hit_sound",
    "EffectModule::request_post_effect_line_circle",
    "EffectModule::remove_post_effect_line",
    "WorkModule::set_int64",
    "KineticModule::change_kinetic",
    "KineticModule::suspend_energy",
    "KineticModule::resume_energy",
    "KineticModule::enable_energy",
    "KineticModule::unable_energy",
    "KineticModule::add_speed",
    "KineticModule::set_consider_ground_friction",
    "MotionModule::set_rate",
    "MotionModule::set_rate_partial",
    "MotionModule::set_frame_partial",
    "MotionModule::set_helper_calculation",
    "WorkModule::inc_int",
    "macros::SET_SPEED_EX",
    "macros::CORRECT",
    "macros::FT_CATCH_STOP",
    "macros::FT_START_ADJUST_MOTION_FRAME_arg1",
    "macros::CLR_SPEED",
    "WorkModule::enable_transition_term_group",
    "WorkModule::unable_transition_term_group_ex",
    "ArticleModule::add_motion_partial",
];

/// Marks a wire hash as the name of a game constant. **Must match the plugin.**
const CONST_BIT: u64 = 1 << 63;

/// One argument of a block's game call, as the export would write it.
fn generic_arg(token: &str) -> Option<A> {
    let token = token.trim();
    match token {
        "true" => return Some(A::Bool(true)),
        "false" => return Some(A::Bool(false)),
        _ => {}
    }
    if let Some(start) = token.find('"') {
        let name = token[start + 1..].split('"').next()?;
        return Some(A::Hash(hash40::hash40(name).0));
    }
    if let Some(start) = token.find('*') {
        let name: String = token[start + 1..]
            .chars()
            .take_while(|letter| letter.is_ascii_alphanumeric() || *letter == '_')
            .collect();
        return Some(A::Hash(hash40::hash40(&name).0 | CONST_BIT));
    }
    if let Ok(value) = token.parse::<i64>() {
        return Some(A::Int(value));
    }
    token.parse::<f32>().ok().map(A::Num)
}

/// A vector argument, like `Vector2::new(1.0, 2.0)` or `&Vector2f{x: 1.0, y: 2.0}`, sent as
/// its numbers in order.
fn vector_parts(token: &str) -> Option<Vec<A>> {
    if !token.contains("Vector") {
        return None;
    }
    let open = token.find(['(', '{'])?;
    let inner = token[open + 1..].trim_end().trim_end_matches([')', '}']);
    inner
        .split(',')
        .map(|part| {
            let part = part.rsplit(':').next()?.trim();
            part.parse::<f32>().ok().map(A::Num)
        })
        .collect()
}

/// A block whose one-line game call is in [`GENERIC`].
fn generic_step(command: &ExcuteStmt) -> Option<(String, Vec<A>)> {
    let lines = crate::acmd::preview_excute_stmts(std::slice::from_ref(command));
    let [line] = lines.as_slice() else {
        return None;
    };
    let (name, rest) = line.trim().split_once('(')?;
    let name = GENERIC.iter().find(|known| **known == name)?;
    let inner = rest.trim_end().strip_suffix(");")?;
    let mut tokens = Vec::new();
    let (mut depth, mut quoted, mut start) = (0, false, 0);
    for (at, letter) in inner.char_indices() {
        match letter {
            '"' => quoted = !quoted,
            '(' | '{' if !quoted => depth += 1,
            ')' | '}' if !quoted => depth -= 1,
            ',' if !quoted && depth == 0 => {
                tokens.push(&inner[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    tokens.push(&inner[start..]);
    // The first token is `agent` or `agent.module_accessor`.
    let args = tokens
        .into_iter()
        .skip(1)
        .filter(|token| !token.trim().is_empty())
        .map(|token| match vector_parts(token) {
            Some(parts) => Some(parts),
            None => generic_arg(token).map(|arg| vec![arg]),
        })
        .collect::<Option<Vec<_>>>()?
        .concat();
    Some(((*name).to_owned(), args))
}

/// True when this command has some way to reach a running match.
pub fn reaches_match(fighter: &str, command: &ExcuteStmt) -> bool {
    step_for(fighter, command).is_some()
}

/// A condition the plugin can check in the match. `None` means this one cannot be checked live.
fn guard_for(condition: &MoveCondition) -> Option<LiveGuard> {
    let compare = |op: NativeComparison| match op {
        NativeComparison::Lt => -2,
        NativeComparison::Lte => -1,
        NativeComparison::Eq => 0,
        NativeComparison::Gte => 1,
        NativeComparison::Gt => 2,
        NativeComparison::Ne => 3,
    };
    Some(match condition {
        MoveCondition::Grounded => LiveGuard::Grounded,
        MoveCondition::Airborne => LiveGuard::Airborne,
        MoveCondition::ItemHeld => LiveGuard::ItemHeld,
        MoveCondition::NativeLrNegative => LiveGuard::FacingLeft,
        MoveCondition::Stick {
            axis,
            op,
            value,
            relative_to_facing,
        } => LiveGuard::Stick {
            vertical: matches!(axis, super::model::StickAxis::Y),
            op: compare(*op),
            value: *value,
            facing: *relative_to_facing,
        },
        MoveCondition::ButtonHeld { button } => LiveGuard::Button(format!("{button:?}")),
        MoveCondition::AttackInfliction { mask, .. } => match mask {
            super::model::InflictionMask::Hit => LiveGuard::AttackHit,
            super::model::InflictionMask::Shield => LiveGuard::AttackShield,
        },
        MoveCondition::WorkFlag { symbol, expected } => LiveGuard::Flag {
            symbol: hash40::hash40(symbol.trim_start_matches('*')).0,
            expected: *expected,
        },
        MoveCondition::WorkInt {
            symbol,
            op,
            value: NativeIntOperand::Literal(value),
        } => LiveGuard::Int {
            symbol: hash40::hash40(symbol.trim_start_matches('*')).0,
            op: compare(*op),
            value: *value,
        },
        MoveCondition::WorkInt {
            symbol,
            op,
            value: NativeIntOperand::FighterKind(owner),
        } => LiveGuard::Int {
            symbol: hash40::hash40(symbol.trim_start_matches('*')).0,
            op: compare(*op),
            value: crate::gimmicks::registry::fighter_kind_value(owner)?,
        },
        MoveCondition::WorkInt {
            symbol,
            op,
            value: NativeIntOperand::Choice(choice),
        } => LiveGuard::IntNamed {
            symbol: hash40::hash40(symbol.trim_start_matches('*')).0,
            op: compare(*op),
            value: hash40::hash40(choice.trim_start_matches('*')).0,
        },
        MoveCondition::WorkFloat { symbol, op, value } => LiveGuard::Float {
            symbol: hash40::hash40(symbol.trim_start_matches('*')).0,
            op: compare(*op),
            value: *value,
        },
        MoveCondition::And { left, right } => {
            LiveGuard::All(vec![guard_for(left)?, guard_for(right)?])
        }
        MoveCondition::Or { left, right } => {
            LiveGuard::Any(vec![guard_for(left)?, guard_for(right)?])
        }
        MoveCondition::Not(inner) => LiveGuard::Not(Box::new(guard_for(inner)?)),
        MoveCondition::Group(inner) => guard_for(inner)?,
        MoveCondition::IsExecute => LiveGuard::All(Vec::new()),
        MoveCondition::CharacterValue {
            chart_id,
            var,
            op,
            value,
        } => LiveGuard::Value {
            chart: chart_id.clone(),
            var: var.clone(),
            op: compare(*op),
            value: match value {
                super::model::CharacterValue::Int(value) => *value as f32,
                super::model::CharacterValue::Float(value) => *value,
                super::model::CharacterValue::Flag(value) => f32::from(u8::from(*value)),
            },
        },
        MoveCondition::HoldingItemKind { item } => LiveGuard::Item {
            symbol: hash40::hash40(item.trim_start_matches('*')).0,
        },
        MoveCondition::MotionChanging => LiveGuard::AnimationChanging,
        MoveCondition::InStatus { symbol } => LiveGuard::Status {
            symbol: hash40::hash40(symbol.trim_start_matches('*')).0,
        },
        MoveCondition::ArticleExists { symbol } => LiveGuard::Article {
            symbol: hash40::hash40(symbol.trim_start_matches('*')).0,
        },
        _ => return None,
    })
}

/// True when this condition can be checked in a running match.
pub fn guard_reaches_match(condition: &MoveCondition) -> bool {
    guard_for(condition).is_some()
}

fn both(path: &Option<LiveGuard>, next: LiveGuard) -> Option<LiveGuard> {
    Some(match path {
        Some(path) => LiveGuard::All(vec![path.clone(), next]),
        None => next,
    })
}

fn walk(
    fighter: &str,
    nodes: &[MoveNode],
    frame: &mut f32,
    guard: &Option<LiveGuard>,
    out: &mut Vec<Flat>,
) {
    for node in nodes {
        let at = |frame: f32| frame.round().max(0.0) as u32;
        match &node.kind {
            NodeKind::Frame { frame: to } | NodeKind::NativeExecuteFrame { frame: to } => {
                *frame = frame.max(*to)
            }
            NodeKind::Wait { frames } => *frame += frames.max(0.0),
            NodeKind::MotionRate { rate } => out.push(Flat {
                frame: at(*frame),
                guard: guard.clone(),
                step: Some(("FT_MOTION_RATE".to_owned(), vec![A::Num(*rate)])),
            }),
            NodeKind::EffectExecute { commands } => {
                out.extend(commands.iter().map(|_| Flat {
                    frame: at(*frame),
                    guard: guard.clone(),
                    step: None,
                }));
            }
            NodeKind::Execute { commands } => {
                for command in commands {
                    out.push(Flat {
                        frame: at(*frame),
                        guard: guard.clone(),
                        step: step_for(fighter, command),
                    });
                }
            }
            NodeKind::Branch {
                condition,
                then_nodes,
                else_nodes,
            } => {
                // A condition the plugin cannot check leaves the original script's call alone.
                let Some(check) = guard_for(condition) else {
                    continue;
                };
                let start = *frame;
                let mut other = start;
                walk(fighter, then_nodes, frame, &both(guard, check.clone()), out);
                walk(
                    fighter,
                    else_nodes,
                    &mut other,
                    &both(guard, LiveGuard::Not(Box::new(check))),
                    out,
                );
                *frame = frame.max(other);
            }
            NodeKind::Repeat {
                body,
                count,
                motion_loop,
            } => {
                // A looping animation replays the block itself; the plugin fires once per loop.
                let times = if motion_loop.is_loop() {
                    1
                } else {
                    (*count).max(1)
                };
                for _ in 0..times {
                    walk(fighter, body, frame, guard, out);
                }
            }
            _ => {}
        }
    }
}

fn flat_track(fighter: &str, graph: &MoveGraph, lane: Track) -> Vec<Flat> {
    let mut out = Vec::new();
    for track in graph.tracks.iter().filter(|track| track.track == lane) {
        walk(fighter, &track.nodes, &mut 0.0, &None, &mut out);
    }
    out
}

fn flat(fighter: &str, graph: &MoveGraph) -> Vec<Flat> {
    flat_track(fighter, graph, Track::Game)
}

/// The plugin's key for a removed call. **Must match the plugin's `drop_key`.**
fn drop_key(name: &str) -> u64 {
    hash40::hash40(&name.to_ascii_lowercase()).0
}

/// Plugin rule category that suppresses a removed call of this step.
fn suppress_category(command: &str) -> Option<u8> {
    use crate::game_link as link;
    Some(match command {
        "SET_SPEED" => link::CAT_SPEED,
        "ADD_SPEED_NO_LIMIT" => link::CAT_ADD_SPEED_NO_LIMIT,
        "SET_AIR" => link::CAT_SET_AIR,
        "REVERSE_LR" => link::CAT_REVERSE_LR,
        "clear_speed_all" => link::CAT_KINETIC_CLEAR_SPEED_ALL,
        "generate_article" => link::CAT_ARTICLE_GENERATE,
        "on_flag" | "off_flag" => link::CAT_WORK_FLAG,
        "enable_term" | "unable_term" => link::CAT_WORK_TRANSITION_TERM,
        _ => return None,
    })
}

/// Rules that make the running match follow `graph` where the game's `source` script differs.
pub fn rules(
    fighter: &str,
    motion: u64,
    source: &crate::data::AcmdScript,
    graph: &MoveGraph,
) -> Vec<HitboxRuleWire> {
    let mut before = flat(
        fighter,
        &MoveGraph {
            tracks: vec![super::timeline::import_for_fighter(
                fighter,
                Track::Game,
                source,
            )],
            ..MoveGraph::default()
        },
    );
    let mut out = Vec::new();
    for now in flat(fighter, graph) {
        let Some((command, args)) = now.step.clone() else {
            continue;
        };
        if let Some(index) = before
            .iter()
            .position(|was| was.frame == now.frame && was.step == now.step)
        {
            before.remove(index);
            continue;
        }
        out.push(HitboxRuleWire {
            motion,
            category: CAT_NODE_STEP,
            hitbox_id: None,
            suppress: false,
            frame_start: None,
            frame_end: None,
            overrides: None,
            inject: Some(InjectRuleWire {
                frame: crate::data::script_to_motion_frame(now.frame),
                args,
                command: Some(command),
                guard: now.guard.clone(),
            }),
            func: None,
        });
    }
    let kept = flat(fighter, graph);
    for was in before {
        let Some((command, args)) = was.step else {
            continue;
        };
        let category = match suppress_category(&command) {
            Some(category) => category,
            // A gimmick message is never in the game's own script.
            None if command.starts_with("event") => continue,
            None => {
                let center = crate::data::script_to_motion_frame(was.frame);
                out.push(HitboxRuleWire {
                    motion,
                    category: crate::game_link::CAT_DROP,
                    hitbox_id: Some(drop_key(&command)),
                    suppress: true,
                    frame_start: Some(center - 1.0),
                    frame_end: Some(center + 1.0),
                    overrides: None,
                    inject: None,
                    func: None,
                });
                continue;
            }
        };
        let center = crate::data::script_to_motion_frame(was.frame);
        // The game's frame drifts from the script's under an animation-speed call. With no spawn
        // of this object left in the move, stop it wherever it comes.
        let anywhere = command == "generate_article"
            && !kept.iter().any(|now| {
                now.step
                    .as_ref()
                    .is_some_and(|(name, list)| *name == command && list.first() == args.first())
            });
        out.push(HitboxRuleWire {
            motion,
            category,
            hitbox_id: match args.first() {
                Some(A::Int(id)) if command == "generate_article" => Some(*id as u64),
                _ => None,
            },
            suppress: true,
            frame_start: (!anywhere).then_some(center - 1.0),
            frame_end: (!anywhere).then_some(center + 1.0),
            overrides: None,
            inject: None,
            func: None,
        });
    }
    out
}

fn frame_guards(all: &[Flat]) -> Vec<(f32, LiveGuard)> {
    all.iter()
        .filter_map(|flat| Some((flat.frame, flat.guard.clone()?)))
        .filter(|(frame, _)| {
            !all.iter()
                .any(|other| other.frame == *frame && other.guard.is_none())
        })
        .map(|(frame, guard)| (crate::data::script_to_motion_frame(frame), guard))
        .collect()
}

/// The If conditions on each frame of a move, so the older families' added calls can carry them.
pub fn guards(fighter: &str, graph: &MoveGraph) -> Vec<(f32, LiveGuard)> {
    frame_guards(&flat(fighter, graph))
}

/// The same for the Visuals lane.
pub fn effect_guards(fighter: &str, graph: &MoveGraph) -> Vec<(f32, LiveGuard)> {
    frame_guards(&flat_track(fighter, graph, Track::Effect))
}

/// Frames where the user put blocks inside an If that the game's script runs unconditionally.
pub fn newly_guarded_frames(
    fighter: &str,
    graph: &MoveGraph,
    source: &crate::data::AcmdScript,
) -> Vec<u32> {
    let before = flat(
        fighter,
        &MoveGraph {
            tracks: vec![super::timeline::import_for_fighter(
                fighter,
                Track::Game,
                source,
            )],
            ..MoveGraph::default()
        },
    );
    let mut frames: Vec<u32> = flat(fighter, graph)
        .into_iter()
        .filter(|now| now.guard.is_some())
        .map(|now| now.frame)
        .filter(|frame| {
            before
                .iter()
                .any(|was| was.frame == *frame && was.guard.is_none())
        })
        .collect();
    frames.dedup();
    frames
}

/// Every graphic a Visuals lane can spawn, including inside If and Repeat blocks and in the lines
/// it keeps as the game has them.
fn lane_spawns(nodes: &[MoveNode], out: &mut std::collections::BTreeSet<String>) {
    use super::model::EffectCommandSlot as Slot;
    for node in nodes {
        match &node.kind {
            NodeKind::EffectExecute { commands } => command_spawns(commands, out),
            NodeKind::MixedEffectExecute { commands, .. } => command_spawns(
                commands.iter().map(|slot| match slot {
                    Slot::Editable(command) | Slot::Preserved(command) => command,
                }),
                out,
            ),
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => {
                lane_spawns(then_nodes, out);
                lane_spawns(else_nodes, out);
            }
            NodeKind::Repeat { body, .. } => lane_spawns(body, out),
            NodeKind::BareCommand { command } => stmt_spawns([command.as_ref()], out),
            NodeKind::Execute { commands } => stmt_spawns(commands, out),
            NodeKind::Preserved { statement } => {
                let lines = crate::acmd::emit_stmts(std::slice::from_ref(statement), "");
                lines.iter().for_each(|line| line_spawns(line, out));
            }
            _ => {}
        }
    }
}

fn stmt_spawns<'a>(
    commands: impl IntoIterator<Item = &'a ExcuteStmt>,
    out: &mut std::collections::BTreeSet<String>,
) {
    for command in commands {
        if let ExcuteStmt::Effect(command) = command {
            command_spawns([command.as_ref()], out);
        }
    }
}

fn statement_spawns(
    stmts: &[crate::data::EffectStmt],
    out: &mut std::collections::BTreeSet<String>,
) {
    use crate::data::EffectStmt as E;
    for stmt in stmts {
        match stmt {
            E::BareEffect(command) => command_spawns([command.as_ref()], out),
            E::Excute(commands) => command_spawns(commands, out),
            E::Loop { body, .. } | E::Cond { body, .. } => statement_spawns(body, out),
            E::Raw(line) => line_spawns(line, out),
            _ => {}
        }
    }
}

fn command_spawns<'a>(
    commands: impl IntoIterator<Item = &'a crate::data::EffectMacro>,
    out: &mut std::collections::BTreeSet<String>,
) {
    for command in commands {
        match command {
            crate::data::EffectMacro::Effect { effect_name, .. } => {
                out.insert(effect_name.to_lowercase());
            }
            crate::data::EffectMacro::Raw(line) => line_spawns(line, out),
            _ => {}
        }
    }
}

/// The graphics a line of code spawns. A line that only ends one names it without playing it.
fn line_spawns(code: &str, out: &mut std::collections::BTreeSet<String>) {
    for line in code.lines() {
        let call = line.split_once("macros::").map_or(line, |(_, call)| call);
        let Some((function, args)) = call.split_once('(') else {
            continue;
        };
        if crate::acmd::effect_spawn_layout(function.trim()).is_none() {
            continue;
        }
        if let Some((name, _)) = args
            .split_once("Hash40::new(\"")
            .and_then(|(_, rest)| rest.split_once('"'))
        {
            out.insert(name.to_lowercase());
        }
    }
}

/// The graphics an effect script spawns, at any depth.
fn script_spawns(script: &crate::data::EffectScript) -> std::collections::BTreeSet<String> {
    let mut out = Default::default();
    statement_spawns(&script.stmts, &mut out);
    out
}

/// The graphics the open move's own scripts spawn and its blocks no longer do.
pub fn removed_spawns(kept: &std::collections::BTreeSet<String>, body: &str) -> Vec<String> {
    let source = script_spawns(&crate::acmd::parse_effect_script(body));
    source.difference(kept).cloned().collect()
}

/// The graphics a move's blocks play, or nothing while they leave its effects to the game.
/// `script` is what they write, when they own its effects: a move whose effects no block can hold
/// has no lane in its graph.
pub fn kept_spawns(
    graph: Option<&MoveGraph>,
    script: Option<&crate::data::EffectScript>,
) -> Option<std::collections::BTreeSet<String>> {
    if let Some(script) = script {
        return Some(script_spawns(script));
    }
    let mut lanes = graph?
        .tracks
        .iter()
        .filter(|track| track.track == Track::Effect)
        .peekable();
    lanes.peek()?;
    let mut kept = Default::default();
    lanes.for_each(|track| lane_spawns(&track.nodes, &mut kept));
    Some(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_one_line_block_call_becomes_a_named_step() {
        let command = ExcuteStmt::AttackModuleAddReactionFrame { id: 2, frames: 4.5 };
        let (name, args) = step_for("mario", &command).unwrap();
        assert_eq!(name, "AttackModule::set_add_reaction_frame");
        assert_eq!(args, vec![A::Int(2), A::Num(4.5), A::Bool(false)]);
        assert_eq!(
            generic_arg("*FOO as u32"),
            Some(A::Hash(hash40::hash40("FOO").0 | CONST_BIT))
        );
        assert_eq!(
            generic_arg("hash40(\"body\") as i64"),
            Some(A::Hash(hash40::hash40("body").0))
        );
    }

    const FIREBALL: &str = "unsafe extern \"C\" fn game_specialn(agent: &mut L2CAgentBase) {\n    macros::FT_MOTION_RATE(agent, 1.15);\n    frame(agent.lua_state_agent, 14.0);\n    if macros::is_excute(agent) {\n        ArticleModule::generate_article(agent.module_accessor, *FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL, false, -1);\n    }\n}\n";

    fn source() -> crate::data::AcmdScript {
        crate::acmd::parse_acmd_script(FIREBALL)
    }

    fn imported() -> MoveGraph {
        MoveGraph {
            tracks: vec![crate::move_nodes::timeline::import_for_fighter(
                "mario",
                Track::Game,
                &source(),
            )],
            ..MoveGraph::default()
        }
    }

    #[test]
    fn an_untouched_move_sends_nothing() {
        assert_eq!(rules("mario", 1, &source(), &imported()).len(), 0);
    }

    /// A move whose blocks do not change its effects keeps no effect lane. Taking that for "no
    /// effects kept" stopped every effect of any move with a node edit in the match.
    #[test]
    fn effects_the_blocks_leave_alone_are_not_removed() {
        let effects = crate::acmd::parse_effect_script(
            "unsafe extern \"C\" fn effect_attackhi3(agent: &mut L2CAgentBase) {\n    frame(agent.lua_state_agent, 4.0);\n    if macros::is_excute(agent) {\n        macros::EFFECT_FOLLOW(agent, Hash40::new(\"sys_attack_arc\"), Hash40::new(\"top\"), 0, 8, 0, 0, 0, 0, 1.0, true);\n    }\n}\n",
        );
        let none = crate::acmd::parse_effect_script(
            "unsafe extern \"C\" fn effect_attackhi3(agent: &mut L2CAgentBase) {\n}\n",
        );
        let source = script_spawns(&effects);
        assert_eq!(source.len(), 1);
        // Blocks of the hitboxes only, and no blocks at all.
        assert_eq!(kept_spawns(Some(&imported()), None), None);
        assert_eq!(kept_spawns(None, None), None);
        // Blocks that own the effects: in a lane, an emptied one, or a script no block can hold.
        let mut owned = imported();
        owned
            .tracks
            .push(crate::move_nodes::effects::import_for_fighter(
                "mario", &effects,
            ));
        assert_eq!(kept_spawns(Some(&owned), None), Some(source.clone()));
        // What they write is newer than a lane that could not be made to write it.
        assert_eq!(
            kept_spawns(Some(&owned), Some(&none)),
            Some(Default::default())
        );
        owned.tracks.last_mut().unwrap().nodes.clear();
        assert_eq!(kept_spawns(Some(&owned), None), Some(Default::default()));
        assert_eq!(
            kept_spawns(Some(&imported()), Some(&none)),
            Some(Default::default())
        );
        assert_eq!(kept_spawns(None, Some(&effects)), Some(source));
    }

    /// An effect the blocks still play under a condition no block can hold, or inside lines kept
    /// as the game has them, is not removed; one they only end is.
    #[test]
    fn an_effect_is_kept_wherever_the_move_still_plays_it() {
        let nested = crate::acmd::parse_effect_script(
            "unsafe extern \"C\" fn effect_specials(agent: &mut L2CAgentBase) {\n    frame(agent.lua_state_agent, 4.0);\n    if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_BAYONETTA_STATUS_WORK_ID_SPECIAL_S_FLAG_NEAR_CLIFF) {\n        if macros::is_excute(agent) {\n            macros::EFFECT(agent, Hash40::new(\"sys_whirlwind_l\"), Hash40::new(\"top\"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, true);\n            macros::EFFECT_OFF_KIND(agent, Hash40::new(\"sys_gone\"), false, true);\n        }\n    }\n}\n",
        );
        let names = |names: &[&str]| names.iter().map(|name| name.to_string()).collect();
        assert_eq!(script_spawns(&nested), names(&["sys_whirlwind_l"]));
        // Nobody's fighter, so the condition is kept as the game has it.
        let lane = crate::move_nodes::effects::import_for_fighter("", &nested);
        let graph = MoveGraph {
            tracks: vec![lane],
            ..MoveGraph::default()
        };
        assert_eq!(
            kept_spawns(Some(&graph), None),
            Some(names(&["sys_whirlwind_l"]))
        );
        // Only what the move itself spawns can have been removed from it.
        let body = "unsafe extern \"C\" fn effect_specials(agent: &mut L2CAgentBase) {\n    if macros::is_excute(agent) {\n        macros::EFFECT(agent, Hash40::new(\"sys_whirlwind_l\"), Hash40::new(\"top\"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, true);\n        macros::EFFECT(agent, Hash40::new(\"sys_hit\"), Hash40::new(\"top\"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, true);\n    }\n}\n";
        let kept = names(&["sys_whirlwind_l", "another_moves"]);
        assert_eq!(removed_spawns(&kept, body), ["sys_hit"]);
        assert!(removed_spawns(&Default::default(), "").is_empty());
        let mut raw = Default::default();
        line_spawns(
            "macros::FOOT_EFFECT(agent, Hash40::new(\"sys_Dash_smoke\"), Hash40::new(\"top\"), 0);\nmacros::EFFECT_OFF_KIND(agent, Hash40::new(\"sys_gone\"), false, true);",
            &mut raw,
        );
        assert_eq!(raw, names(&["sys_dash_smoke"]));
    }

    #[test]
    fn every_removable_call_has_a_plugin_hook() {
        let plugin = [
            include_str!("../../plugins/slight_replica/src/slight/hitbox_viewer/drop_hooks.rs"),
            include_str!("../../plugins/slight_replica/src/slight/hitbox_viewer/mod.rs"),
            include_str!("../../plugins/slight_replica/src/slight/hitbox_viewer/rate_hooks.rs"),
        ]
        .concat();
        let special = [
            "set_whole",
            "hit_all_normal",
            "start_cutin",
            "flash_eye",
            "jostle",
            "camera_zoom_out",
            "slow_whole",
            "slow_clear",
            "shoot_article",
            "remove_article",
            "article_visibility",
        ];
        for name in GENERIC.iter().chain(&special) {
            assert!(
                suppress_category(name).is_some()
                    || plugin.contains(&format!("\"{name}\"]"))
                    || plugin.contains(&format!("\"{name}\", ")),
                "removing {name} from a move would not reach the match"
            );
        }
    }

    /// A block changed through its code reaches the match the way one changed in its fields
    /// does: both compile to the same script, which is what the live hitbox rules come from.
    #[test]
    fn a_hitbox_edited_as_code_matches_the_same_edit_in_its_fields() {
        let jab = "unsafe extern \"C\" fn game_attack11(agent: &mut L2CAgentBase) {\n    frame(agent.lua_state_agent, 2.0);\n    if macros::is_excute(agent) {\n        macros::ATTACK(agent, 0, 0, Hash40::new(\"top\"), 3.0, 361, 25, 0, 20, 2.0, 0.0, 5.0, 6.0, None, None, None, 1.0, 1.0, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new(\"collision_attr_normal\"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_PUNCH, *ATTACK_REGION_PUNCH);\n    }\n}\n";
        let source = crate::acmd::parse_acmd_script(jab);
        let unchanged = MoveGraph {
            tracks: vec![crate::move_nodes::timeline::import_for_fighter(
                "mario",
                Track::Game,
                &source,
            )],
            ..MoveGraph::default()
        };
        let block = unchanged.tracks[0]
            .nodes
            .iter()
            .find(|node| matches!(node.kind, NodeKind::Execute { .. }))
            .unwrap()
            .clone();
        let code = crate::move_nodes::timeline::block_code("mario", &unchanged.tracks[0], &block);
        assert!(code.contains("3.0, 361"), "{code}");
        let mut by_code = unchanged.clone();
        let mut replacement = crate::move_nodes::timeline::blocks_from_code(
            "mario",
            Track::Game,
            &code.replace("3.0, 361", "9.0, 361"),
        )
        .unwrap();
        replacement[0].id = block.id;
        let nodes = &mut by_code.tracks[0].nodes;
        let index = nodes.iter().position(|node| node.id == block.id).unwrap();
        nodes.splice(index..=index, replacement);
        let mut by_field = unchanged.clone();
        for node in &mut by_field.tracks[0].nodes {
            if let NodeKind::Execute { commands } = &mut node.kind {
                for command in commands {
                    if let crate::data::ExcuteStmt::Attack(attack) = command {
                        attack.damage = 9.0;
                    }
                }
            }
        }
        let script = |graph: &MoveGraph| {
            let compiled =
                crate::move_nodes::timeline::compile("mario", "attack11", graph).unwrap();
            crate::acmd::preview_game_fn(&compiled[0].1, "attack11")
        };
        assert_ne!(script(&by_field), script(&unchanged));
        assert_eq!(script(&by_code), script(&by_field));
        let hitboxes = crate::acmd::parse_acmd_script(&script(&by_code)).to_hitboxes();
        assert_eq!(hitboxes[0].damage, 9.0);
    }

    #[test]
    fn a_removed_spawn_is_stopped_for_the_whole_move() {
        let mut graph = imported();
        graph.tracks[0].nodes.clear();
        let rules = rules("mario", 1, &source(), &graph);
        let stop = rules
            .iter()
            .find(|rule| rule.category == crate::game_link::CAT_ARTICLE_GENERATE)
            .unwrap();
        assert!(stop.suppress && stop.frame_start.is_none());
    }
}

/// Every fighter's real moves, from a local copy of the script dump the editor fetches from.
/// Set `VISIONARY_SCRIPT_DUMP` to the dump's `smashline` folder to run it.
#[cfg(test)]
mod every_fighter_tests {
    use super::super::model::*;

    #[test]
    fn every_fighters_moves_load_and_send_nothing_until_edited() {
        let Some(root) = std::env::var_os("VISIONARY_SCRIPT_DUMP") else {
            return;
        };
        let mut problems = Vec::new();
        let mut moves = 0;
        let mut fighters = 0;
        for kind in crate::gimmicks::registry::FIGHTER_KINDS {
            let fighter = kind.fighter;
            let dir = std::path::Path::new(&root)
                .join(format!("lua2cpp_{fighter}"))
                .join(fighter);
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            fighters += 1;
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(body) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let source =
                    crate::acmd::parse_acmd_script(&crate::acmd::script_source_from_body(&body));
                let name = path.file_stem().unwrap().to_string_lossy().to_string();
                let result = std::panic::catch_unwind(|| {
                    let track =
                        super::super::timeline::import_for_fighter(fighter, Track::Game, &source);
                    let mut fingerprints = Vec::new();
                    super::super::live_support::source_fingerprints(
                        &track.nodes,
                        &mut fingerprints,
                    );
                    let graph = MoveGraph {
                        tracks: vec![track],
                        ..MoveGraph::default()
                    };
                    let sent = super::rules(fighter, 1, &source, &graph);
                    let waiting =
                        super::super::live_support::not_live_blocks(fighter, &graph, &fingerprints);
                    (sent.len(), waiting)
                });
                moves += 1;
                match result {
                    Err(_) => problems.push(format!("{fighter}/{name}: import panicked")),
                    Ok((sent, _)) if sent > 0 => problems.push(format!(
                        "{fighter}/{name}: unedited move sends {sent} rule(s)"
                    )),
                    Ok((_, waiting)) if !waiting.is_empty() => {
                        problems.push(format!("{fighter}/{name}: not live {waiting:?}"))
                    }
                    Ok(_) => {}
                }
            }
        }
        println!("checked {moves} moves across {fighters} fighters");
        assert!(
            problems.is_empty(),
            "{} problems:\n{}",
            problems.len(),
            problems.join("\n")
        );
    }
}
