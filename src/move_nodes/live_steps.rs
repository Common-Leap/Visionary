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
            _ => return None,
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
                return None;
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
        ExcuteStmt::WorkFlag(call) => {
            let symbol = call.flag.trim_start_matches('*').to_owned();
            let on = matches!(call.action, crate::data::WorkFlagAction::On);
            (
                if on { "on_flag" } else { "off_flag" },
                vec![A::Hash(hash40::hash40(&symbol).0)],
            )
        }
        _ => return None,
    };
    Some((name.to_owned(), args))
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
        let Some(category) = suppress_category(&command) else {
            continue;
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

/// Every graphic a Visuals lane can spawn, including inside If and Repeat blocks.
pub fn spawn_names(nodes: &[MoveNode], out: &mut std::collections::BTreeSet<String>) {
    for node in nodes {
        match &node.kind {
            NodeKind::EffectExecute { commands } => {
                out.extend(commands.iter().filter_map(|command| match command {
                    crate::data::EffectMacro::Effect { effect_name, .. } => {
                        Some(effect_name.clone())
                    }
                    _ => None,
                }));
            }
            NodeKind::MixedEffectExecute { commands, .. } => {
                out.extend(commands.iter().filter_map(|command| match command {
                    super::model::EffectCommandSlot::Editable(
                        crate::data::EffectMacro::Effect { effect_name, .. },
                    ) => Some(effect_name.clone()),
                    _ => None,
                }));
            }
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => {
                spawn_names(then_nodes, out);
                spawn_names(else_nodes, out);
            }
            NodeKind::Repeat { body, .. } => spawn_names(body, out),
            _ => {}
        }
    }
}

/// The graphics the game's own effect script spawns.
pub fn source_spawn_names(
    script: &crate::data::EffectScript,
) -> std::collections::BTreeSet<String> {
    let mut out = Default::default();
    spawn_names(
        &super::effects::import_for_fighter("", script).nodes,
        &mut out,
    );
    out
}

/// Source graphics this move no longer spawns anywhere.
pub fn removed_spawns(
    graph: &MoveGraph,
    source: &std::collections::BTreeSet<String>,
) -> Vec<String> {
    let mut kept = Default::default();
    for track in graph
        .tracks
        .iter()
        .filter(|track| track.track == Track::Effect)
    {
        spawn_names(&track.nodes, &mut kept);
    }
    source.difference(&kept).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
