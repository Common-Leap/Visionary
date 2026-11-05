//! The flat live ACMD protocol cannot represent runtime control flow.
use super::model::{MoveGraph, NodeKind, Track};
use std::collections::BTreeMap;

/// Tracks whose shape a compiled export must keep whole.
#[cfg(test)]
pub fn source_only_tracks(graph: &MoveGraph) -> impl Iterator<Item = Track> + '_ {
    graph.tracks.iter().filter_map(|track| {
        (track.track == Track::Effect
            || track.nodes.iter().any(|node| {
                matches!(
                    node.kind,
                    NodeKind::Branch { .. }
                        | NodeKind::Repeat { .. }
                        | NodeKind::MotionRate { .. }
                        | NodeKind::NativeExecuteFrame { .. }
                        | NodeKind::NativeMotionLoopSync
                        | NodeKind::NativeWaitLoopClear
                        | NodeKind::BareCommand { .. }
                        | NodeKind::MixedExecute { .. }
                        | NodeKind::MixedEffectExecute { .. }
                        | NodeKind::Preserved { .. }
                ) || matches!(&node.kind, NodeKind::Execute { commands } if commands.iter().any(|command| {
                    command_requires_source(command)
                        || (track.track != Track::Game && matches!(command, crate::data::ExcuteStmt::WorkFlag(_) | crate::data::ExcuteStmt::WorkModuleSet(_)))
                        || (track.track == Track::Game && matches!(command, crate::data::ExcuteStmt::Sound(_) | crate::data::ExcuteStmt::Expression(_)))
                        || (track.track == Track::Expression
                            && matches!(command,
                                crate::data::ExcuteStmt::Wind(_)
                                | crate::data::ExcuteStmt::EraseWind(_)
                                | crate::data::ExcuteStmt::HitStatus { .. }
                                | crate::data::ExcuteStmt::HitResetAll))
                }))
            }))
        .then_some(track.track)
    })
}

fn command_requires_source(command: &crate::data::ExcuteStmt) -> bool {
    match command {
        crate::data::ExcuteStmt::NativeCall(_)
        | crate::data::ExcuteStmt::Slope(_)
        | crate::data::ExcuteStmt::ItemVisibility { .. }
        | crate::data::ExcuteStmt::ShootItemBullet
        | crate::data::ExcuteStmt::StartInfoFlashEye
        | crate::data::ExcuteStmt::CameraZoomOut
        | crate::data::ExcuteStmt::StartCutIn
        | crate::data::ExcuteStmt::CaptureCutCommon
        | crate::data::ExcuteStmt::JostleSetStatus { .. }
        | crate::data::ExcuteStmt::FixedJostleArea(_)
        | crate::data::ExcuteStmt::VisibilitySetInt64 { .. }
        | crate::data::ExcuteStmt::VisibilitySetWhole { .. }
        | crate::data::ExcuteStmt::VisibilitySetDefaultInt64 { .. }
        | crate::data::ExcuteStmt::AttackModuleSetAll { .. }
        | crate::data::ExcuteStmt::AttackModuleSetIndexed { .. }
        | crate::data::ExcuteStmt::AttackModuleSetHeightAll { .. }
        | crate::data::ExcuteStmt::AttackModuleSetFinalFinishCutIn { .. }
        | crate::data::ExcuteStmt::AttackModuleInitAttackPos { .. }
        | crate::data::ExcuteStmt::AttackModuleSetAttackReferenceJointId { .. }
        | crate::data::ExcuteStmt::AttackModuleSetNoDamageFlySmokeAll { .. }
        | crate::data::ExcuteStmt::AttackModuleSetDamageShakeScale(_)
        | crate::data::ExcuteStmt::AttackModuleSetNoUniqEffectAll(_)
        | crate::data::ExcuteStmt::AttackModuleSetTargetCategory(_)
        | crate::data::ExcuteStmt::HitModuleSetStatusAllNormal
        | crate::data::ExcuteStmt::AttackModuleSetSize(_)
        | crate::data::ExcuteStmt::AttackModuleSetVecTargetPos(_)
        | crate::data::ExcuteStmt::OptionalHitFeedback(_)
        | crate::data::ExcuteStmt::AttackModuleAddReactionFrame { .. }
        | crate::data::ExcuteStmt::AttackModuleAddReactionFrameRevised { .. }
        | crate::data::ExcuteStmt::AttackAbsApplyThrow
        | crate::data::ExcuteStmt::CheckFinishCamera(_)
        | crate::data::ExcuteStmt::CheckFinalStartCamera(_)
        | crate::data::ExcuteStmt::FinalFearFace(_)
        | crate::data::ExcuteStmt::GrabRebound { .. }
        | crate::data::ExcuteStmt::WorkTransitionTerm(_)
        | crate::data::ExcuteStmt::SlowWhole(_)
        | crate::data::ExcuteStmt::SlowOpponent(_)
        | crate::data::ExcuteStmt::GimmickEvent { .. }
        | crate::data::ExcuteStmt::GimmickChartEvent { .. }
        | crate::data::ExcuteStmt::GimmickChartEventValues { .. }
        | crate::data::ExcuteStmt::ArticleLifecycle(_)
        | crate::data::ExcuteStmt::Expression(
            crate::data::ExpressionCall::ControlModuleSetRumble { .. },
        ) => true,
        crate::data::ExcuteStmt::WorkFlag(call) => super::native_work::is_source_only_flag_symbol(
            call.flag.strip_prefix('*').unwrap_or(&call.flag),
        ),
        crate::data::ExcuteStmt::WorkModuleSet(call) => {
            super::native_work::is_source_only_work_module_set(call)
        }
        _ => false,
    }
}

/// Gate every publication, including watchers, reconnect and project replay. Control flow and
/// unsupported blocks no longer withhold a whole script: the flat rules carry what they can, and
/// [`not_live_blocks`] names the rest. Only an ambiguous fighter name fails closed.
pub fn allows_flat_live_rules(
    graphs: &BTreeMap<String, BTreeMap<String, MoveGraph>>,
    rule_key: &str,
) -> bool {
    let (move_key, family) = match rule_key.split_once('#') {
        Some((move_key, family)) if !family.contains('#') => (move_key, Some(family)),
        Some(_) => return false,
        None => (rule_key, None),
    };
    let Some((fighter, move_name)) = parse_move_key(move_key) else {
        return false;
    };
    if family.is_some_and(|family| hitbox_family_track(family).is_none()) {
        return false;
    }
    unique_move_graph(graphs, fighter, move_name).is_ok()
}

/// Effect rules pass the same identity check.
pub fn allows_flat_effect_rules(
    graphs: &BTreeMap<String, BTreeMap<String, MoveGraph>>,
    rule_key: &str,
) -> bool {
    parse_move_key(rule_key)
        .is_some_and(|(fighter, move_name)| unique_move_graph(graphs, fighter, move_name).is_ok())
}

/// `Ok(None)` means no graph for this fighter/move (legacy rules remain eligible); `Err(())`
/// means fighter identity is ambiguous and must fail closed.
fn unique_move_graph<'a>(
    graphs: &'a BTreeMap<String, BTreeMap<String, MoveGraph>>,
    fighter: &str,
    move_name: &str,
) -> Result<Option<&'a MoveGraph>, ()> {
    let mut matches = graphs
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(fighter));
    let Some((_, moves)) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(());
    }
    Ok(moves.get(move_name))
}

fn parse_move_key(key: &str) -> Option<(&str, &str)> {
    let (fighter, move_name) = key.split_once('/')?;
    if move_name.contains('/') || !is_identifier(fighter) || !is_identifier(move_name) {
        return None;
    }
    Some((fighter, move_name))
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn hitbox_family_track(family: &str) -> Option<Track> {
    Some(match family {
        "sound" => Track::Sound,
        "expression"
        | "expression_motion_module_set_frame_partial"
        | "expression_motion_module_set_rate_partial" => Track::Expression,
        "atkmod"
        | "node_steps"
        | "add_speed_no_limit"
        | "change_kinetic"
        | "clr_speed"
        | "correct"
        | "ft_catch_stop"
        | "ft_start_adjust_motion_frame"
        | "hurt"
        | "kinetic_add_speed"
        | "kinetic_clear_speed_all"
        | "kinetic_energy"
        | "kinetic_set_consider_ground_friction"
        | "motion_module_set_frame_partial"
        | "motion_module_set_helper_calculation"
        | "motion_module_set_rate"
        | "motion_module_set_rate_partial"
        | "rate"
        | "reverse_lr"
        | "set_air"
        | "speed"
        | "speed_ex"
        | "work_flag"
        | "work_module_inc_int"
        | "work_module_set"
        | "work_transition_term" => Track::Game,
        _ => return None,
    })
}

/// The on-screen names of blocks the user added that cannot reach a running match yet. Anything
/// the game's own script already does keeps running by itself, so `source` is left out.
pub fn not_live_blocks(fighter: &str, graph: &MoveGraph, source: &[String]) -> Vec<String> {
    fn walk(
        fighter: &str,
        source: &[String],
        nodes: &[super::model::MoveNode],
        out: &mut Vec<String>,
    ) {
        for node in nodes {
            match &node.kind {
                NodeKind::Execute { commands }
                    if commands.iter().any(|command| {
                        command_requires_source(command)
                            && !super::live_steps::reaches_match(fighter, command)
                            && !source.contains(&format!("{command:?}"))
                    }) =>
                {
                    out.push(super::canvas::story_caption(node));
                }
                NodeKind::Branch {
                    condition,
                    then_nodes,
                    else_nodes,
                } => {
                    // A check imported from the game's script is the game's own and keeps running.
                    if !matches!(condition, super::model::MoveCondition::Source { .. })
                        && !super::live_steps::guard_reaches_match(condition)
                        && !source.contains(&format!("{condition:?}"))
                    {
                        out.push(super::canvas::story_caption(node));
                    }
                    walk(fighter, source, then_nodes, out);
                    walk(fighter, source, else_nodes, out);
                }
                NodeKind::Repeat { body, .. } => walk(fighter, source, body, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    for track in &graph.tracks {
        walk(fighter, source, &track.nodes, &mut out);
    }
    out
}

/// Every command and condition in a set of imported blocks, for [`not_live_blocks`].
pub fn source_fingerprints(nodes: &[super::model::MoveNode], out: &mut Vec<String>) {
    for node in nodes {
        match &node.kind {
            NodeKind::Execute { commands } => {
                out.extend(commands.iter().map(|command| format!("{command:?}")));
            }
            NodeKind::Branch {
                condition,
                then_nodes,
                else_nodes,
            } => {
                out.push(format!("{condition:?}"));
                source_fingerprints(then_nodes, out);
                source_fingerprints(else_nodes, out);
            }
            NodeKind::Repeat { body, .. } => source_fingerprints(body, out),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_flow_no_longer_withholds_a_script_but_ambiguous_names_still_do() {
        let graph = MoveGraph::default();
        let mut graphs = BTreeMap::new();
        graphs.insert(
            "mario".to_owned(),
            BTreeMap::from([("special_n".to_owned(), graph.clone())]),
        );
        assert!(allows_flat_live_rules(&graphs, "mario/special_n"));
        assert!(allows_flat_effect_rules(&graphs, "mario/special_n"));
        assert!(!allows_flat_live_rules(&graphs, "mario/special_n/extra"));
        graphs.insert("MARIO".to_owned(), BTreeMap::new());
        assert!(!allows_flat_live_rules(&graphs, "mario/special_n"));
        assert!(!allows_flat_effect_rules(&graphs, "mario/special_n"));
    }
}
