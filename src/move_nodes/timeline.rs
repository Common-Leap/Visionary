//! Bounded conversion between the move-node timeline and the existing ACMD model.
//!
//! Authored repeats are limited to 100 iterations, nesting to 8 levels, node count to
//! 10,000, commands per execute block to 256, and expanded node/command executions to
//! 100,000. Imports that exceed the editable
//! structure limits remain whole `Preserved` baseline statements where possible.
//!
//! Authored `Execute` nodes accept the closed `ExcuteStmt` variants except `Raw`, transition-term
//! writes, and WorkModule writes outside fighter-owned identifiers or the explicit source-only
//! common flag set. Attack macro selectors, wind macro selectors, sound macro
//! selectors, and quoted bone/sound names are each checked against their exact supported
//! shape before emission.

use std::collections::{HashMap, HashSet};

use syn::{Expr, ExprCall, ExprLit, ExprPath, Lit, UnOp};

use crate::data::{AcmdScript, AcmdStmt, ExcuteStmt};
use crate::gimmicks::registry::{self, IdentifierKind};
use crate::move_nodes::model::{
    EffectCommandSlot, InflictionMask, InflictionQuery, InputButton, MoveCommand, MoveCondition,
    MoveGraph, MoveNode, MoveTrack, NodeError, NodeKind, Track, SCHEMA_VERSION,
};

const MAX_NESTING: usize = 8;
const MAX_NODES: usize = 10_000;
const MAX_REPEAT: u32 = 100;
const MAX_COMMANDS_PER_EXECUTE: usize = 256;
const MAX_EXPANDED_NODES: u64 = 100_000;
const MIXED_EFFECT_WITNESS_PREFIX: &str = "mixed_effect_witness:";
const MEASURED_DIRECT_RUMBLE_TARGET: &str = "*BATTLE_OBJECT_ID_INVALID as u32";

/// Convert one ACMD script into an editable track while retaining unsupported source exactly.
#[cfg(test)]
pub fn import(track: Track, script: &AcmdScript) -> MoveTrack {
    import_inner(None, track, script)
}

/// Import using the fighter's generated constant registry for commands whose identifiers are
/// fighter-owned. Unsupported or unregistered source remains one immutable baseline statement.
pub fn import_for_fighter(fighter: &str, track: Track, script: &AcmdScript) -> MoveTrack {
    import_inner(Some(fighter), track, script)
}

fn import_inner(fighter: Option<&str>, track: Track, script: &AcmdScript) -> MoveTrack {
    // The upper two bits separate independently imported tracks so their IDs remain globally
    // unique when the UI combines them into one graph.
    let mut next_id = match track {
        Track::Game => 1,
        Track::Sound => 1 << 29,
        Track::Expression => 2 << 29,
        Track::Effect => 3 << 29,
    };
    let mut baseline = Vec::new();
    let nodes = import_stmts(
        fighter,
        track,
        &script.stmts,
        &mut next_id,
        0,
        &mut baseline,
        false,
        false,
    );
    MoveTrack {
        track,
        nodes,
        baseline,
        effect_execute_baseline: Vec::new(),
    }
}

fn fresh_id(next_id: &mut u32) -> u32 {
    let id = *next_id;
    *next_id = next_id.saturating_add(1);
    id
}

/// Promote only exact, direct, unattributed named-event statements from an Execute block whose
/// parent control-flow scopes were independently proven editable by this importer.
fn promote_exact_nested_named_event_calls(commands: &[ExcuteStmt]) -> (Vec<ExcuteStmt>, bool) {
    let mut lines = Vec::new();
    let mut command_starts = Vec::with_capacity(commands.len());
    for command in commands {
        let wrapper = AcmdStmt::Excute(vec![command.clone()]);
        let emitted = crate::acmd::emit_stmts(&[wrapper], "");
        let body = emitted
            .get(1..emitted.len().saturating_sub(1))
            .unwrap_or_default();
        command_starts.push(lines.len());
        lines.extend(body.iter().flat_map(|line| line.lines().map(str::to_owned)));
    }
    let line_refs = lines.iter().map(String::as_str).collect::<Vec<_>>();
    let event_statements = crate::acmd::unattributed_gimmick_event_calls(&line_refs);
    let mut promoted_any = false;
    let promoted = commands
        .iter()
        .enumerate()
        .map(|(index, command)| {
            let ExcuteStmt::Raw(_) = command else {
                return command.clone();
            };
            if let Some(event) = event_statements.get(&command_starts[index]) {
                promoted_any = true;
                event.clone()
            } else {
                command.clone()
            }
        })
        .collect();
    (promoted, promoted_any)
}

fn contains_named_event_source(statements: &[AcmdStmt]) -> bool {
    statements.iter().any(|statement| match statement {
        AcmdStmt::Excute(commands) => commands.iter().any(|command| match command {
            ExcuteStmt::GimmickEvent { .. }
            | ExcuteStmt::GimmickChartEvent { .. }
            | ExcuteStmt::GimmickChartEventValues { .. } => true,
            ExcuteStmt::Raw(line) => {
                line.contains("dispatch_named_from_module")
                    || line.contains("dispatch_named_chart_from_module")
                    || line.contains("dispatch_named_chart_values_from_module")
            }
            _ => false,
        }),
        AcmdStmt::Bare(command) => match command.as_ref() {
            ExcuteStmt::GimmickEvent { .. }
            | ExcuteStmt::GimmickChartEvent { .. }
            | ExcuteStmt::GimmickChartEventValues { .. } => true,
            ExcuteStmt::Raw(line) => {
                line.contains("dispatch_named_from_module")
                    || line.contains("dispatch_named_chart_from_module")
                    || line.contains("dispatch_named_chart_values_from_module")
            }
            _ => false,
        },
        AcmdStmt::RawBlock { body, .. } | AcmdStmt::Loop { body, .. } => {
            contains_named_event_source(body)
        }
        _ => false,
    })
}

#[allow(clippy::too_many_arguments)]
fn import_stmts(
    fighter: Option<&str>,
    track: Track,
    stmts: &[AcmdStmt],
    next_id: &mut u32,
    depth: usize,
    baseline: &mut Vec<AcmdStmt>,
    named_event_parent_proven: bool,
    execute_context: bool,
) -> Vec<MoveNode> {
    // Named-event promotion is source-only and independent of native command parsing. A child
    // may be promoted only below an imported, fighter-aware clean Game branch/repeat, then only
    // inside an unattached plain Execute and as an exact direct helper statement.
    let has_event_source = contains_named_event_source(stmts);
    let has_scoped_read_header = stmts.iter().any(|stmt| {
        matches!(stmt, AcmdStmt::RawBlock { header, .. } if header.contains("AttackModule::is_infliction") || header.contains("ItemModule::is_have_item"))
    });
    // Execute guards and source conditions both need to know the branch carries no attribute.
    let has_execute_header = stmts.iter().any(|stmt| {
        matches!(stmt, AcmdStmt::RawBlock { header, .. }
            if header.contains("is_excute") || source_condition(fighter, header).is_some())
    });
    let empty_heads = std::collections::BTreeSet::new();
    let mut clean_branch_heads = std::collections::BTreeSet::new();
    let mut clean_loop_heads = std::collections::BTreeSet::new();
    let mut clean_execute_heads = std::collections::BTreeSet::new();
    if has_event_source || has_scoped_read_header || has_execute_header {
        let mut body_lines = Vec::new();
        let mut statement_lines = Vec::with_capacity(stmts.len());
        for statement in stmts {
            statement_lines.push(body_lines.len());
            body_lines.extend(
                crate::acmd::emit_stmts(std::slice::from_ref(statement), "")
                    .into_iter()
                    .flat_map(|emitted| emitted.lines().map(str::to_owned).collect::<Vec<_>>()),
            );
        }
        let body_refs = body_lines.iter().map(String::as_str).collect::<Vec<_>>();
        let clean_lines = crate::acmd::unattributed_source_branch_lines(&body_refs);
        clean_branch_heads.extend(
            statement_lines
                .iter()
                .enumerate()
                .filter_map(|(index, line)| clean_lines.contains(line).then_some(index)),
        );
        if has_event_source {
            let clean_lines = crate::acmd::unattributed_source_loop_heads(&body_refs);
            clean_loop_heads.extend(
                statement_lines
                    .iter()
                    .enumerate()
                    .filter_map(|(index, line)| clean_lines.contains(line).then_some(index)),
            );
            let clean_lines = crate::acmd::unattributed_plain_execute_heads(&body_refs);
            clean_execute_heads.extend(
                statement_lines
                    .iter()
                    .enumerate()
                    .filter_map(|(index, line)| clean_lines.contains(line).then_some(index)),
            );
        }
    }

    // Keep new SDK-only reads attached to their original statement attributes. Legacy
    // conditions retain their existing policy; malformed scope cannot authorize a new read.
    let scoped_read_branch_heads = if has_scoped_read_header {
        clean_branch_heads.clone()
    } else {
        empty_heads
    };
    let mut out = Vec::with_capacity(stmts.len());
    let mut index = 0;
    while index < stmts.len() {
        let stmt = &stmts[index];
        if depth < MAX_NESTING {
            if let AcmdStmt::RawBlock { header, body } = stmt {
                let adjacent_else_body = stmts.get(index + 1).and_then(|next| match next {
                    AcmdStmt::RawBlock { header, body } if header.trim() == "else {" => {
                        Some(body.as_slice())
                    }
                    _ => None,
                });
                let event_branch_must_be_clean = contains_named_event_source(body)
                    || adjacent_else_body.is_some_and(contains_named_event_source);
                if let Some(condition) = import_condition_for_track(fighter, track, header)
                    .filter(|condition| {
                        (!(contains_attack_infliction(condition) || contains_item_held(condition))
                            || scoped_read_branch_heads.contains(&index))
                            && (!contains_execute_condition(condition)
                                || clean_branch_heads.contains(&index))
                    })
                    .filter(|_| !event_branch_must_be_clean || clean_branch_heads.contains(&index))
                    .or_else(|| {
                        (import_condition(fighter, header).is_none()
                            && !event_branch_must_be_clean
                            && source_branch_allowed(
                                header,
                                &out,
                                clean_branch_heads.contains(&index),
                            ))
                        .then(|| source_condition(fighter, header))
                        .flatten()
                    })
                {
                    let source = matches!(condition, MoveCondition::Source { .. });
                    let paired_else = stmts.get(index + 1).and_then(|next| match next {
                        AcmdStmt::RawBlock { header, body } if header.trim() == "else {" => {
                            Some(body.as_slice())
                        }
                        _ => None,
                    });
                    let followed_by_other_else = stmts.get(index + 1).is_some_and(|next| {
                        matches!(next, AcmdStmt::RawBlock { header, .. } if header.trim() != "else {" && header.trim_start().starts_with("else"))
                    });
                    // An empty else has no representation in MoveCondition today. Keep the
                    // original pair intact instead of dropping that syntactic distinction.
                    // A source header needs no pair: the else stays its own source branch.
                    let paired_else = paired_else.filter(|body| !(source && body.is_empty()));
                    if source
                        || !followed_by_other_else
                            && !paired_else.is_some_and(|body| body.is_empty())
                    {
                        if let MoveCondition::Source { header } = &condition {
                            baseline.push(source_condition_witness(header));
                        }
                        let then_nodes = import_stmts(
                            fighter,
                            track,
                            body,
                            next_id,
                            depth + 1,
                            baseline,
                            !source && track == Track::Game && clean_branch_heads.contains(&index),
                            execute_context || matches!(condition, MoveCondition::IsExecute),
                        );
                        let else_nodes = paired_else
                            .map(|body| {
                                import_stmts(
                                    fighter,
                                    track,
                                    body,
                                    next_id,
                                    depth + 1,
                                    baseline,
                                    !source
                                        && track == Track::Game
                                        && clean_branch_heads.contains(&index),
                                    execute_context,
                                )
                            })
                            .unwrap_or_default();
                        out.push(MoveNode {
                            id: fresh_id(next_id),
                            kind: NodeKind::Branch {
                                condition,
                                then_nodes,
                                else_nodes,
                            },
                        });
                        index += if paired_else.is_some() { 2 } else { 1 };
                        continue;
                    }
                }
            }
        }
        let kind = match stmt {
            AcmdStmt::Frame(frame) if frame.is_finite() => NodeKind::Frame { frame: *frame },
            AcmdStmt::NativeMotionLoopSync if depth == 0 && track != Track::Game => {
                NodeKind::NativeMotionLoopSync
            }
            AcmdStmt::NativeExecuteFrame(frame)
                if depth == 0 && track != Track::Sound && valid_native_execute_frame(*frame) =>
            {
                NodeKind::NativeExecuteFrame { frame: *frame }
            }
            AcmdStmt::Wait(frames) if frames.is_finite() => NodeKind::Wait { frames: *frames },
            AcmdStmt::MotionRate(rate) if track == Track::Game && valid_motion_rate(*rate) => {
                NodeKind::MotionRate { rate: *rate }
            }
            AcmdStmt::WaitLoopClear if depth == 0 && wait_loop_clear_track(track) => {
                NodeKind::NativeWaitLoopClear
            }
            // The motion loop is the whole script: its body is imported as the root it replaces.
            AcmdStmt::Loop {
                count: 1,
                body,
                motion_loop,
            } if depth == 0
                && motion_loop_track(*motion_loop, track)
                && index + 1 == stmts.len()
                && !body.iter().any(waits_for_motion_loop) =>
            {
                NodeKind::Repeat {
                    motion_loop: *motion_loop,
                    count: 1,
                    body: import_stmts(
                        fighter,
                        track,
                        body,
                        next_id,
                        depth,
                        baseline,
                        named_event_parent_proven,
                        execute_context,
                    ),
                }
            }
            AcmdStmt::Loop {
                count,
                body,
                motion_loop: crate::data::MotionLoop::No,
            } if depth < MAX_NESTING && *count > 0 && *count <= MAX_REPEAT as usize => {
                NodeKind::Repeat {
                    motion_loop: crate::data::MotionLoop::No,
                    count: *count as u32,
                    body: import_stmts(
                        fighter,
                        track,
                        body,
                        next_id,
                        depth + 1,
                        baseline,
                        track == Track::Game && clean_loop_heads.contains(&index),
                        execute_context,
                    ),
                }
            }
            AcmdStmt::Excute(commands)
                if !commands.is_empty() && commands.len() <= MAX_COMMANDS_PER_EXECUTE =>
            {
                let (mut commands, mut promoted_event) = if named_event_parent_proven
                    && track == Track::Game
                    && clean_execute_heads.contains(&index)
                {
                    promote_exact_nested_named_event_calls(commands)
                } else {
                    (commands.clone(), false)
                };
                // Borrowed instance flags retain source-only calls without disabling the
                // original owner's registered live flag primitive. Update the mixed witness
                // alongside the command representation so immutable neighboring slots stay put.
                for command in &mut commands {
                    if let ExcuteStmt::WorkFlag(call) = command {
                        if fighter.is_some_and(|owner| {
                            super::native_work::is_borrowed_registry_flag(
                                owner,
                                call.flag.strip_prefix('*').unwrap_or(&call.flag),
                            )
                        }) {
                            *command = ExcuteStmt::NativeCall(super::native_calls::NativeCall {
                                callee: match call.action {
                                    crate::data::WorkFlagAction::On => "WorkModule::on_flag",
                                    crate::data::WorkFlagAction::Off => "WorkModule::off_flag",
                                }
                                .into(),
                                arguments: vec![call.flag.clone()],
                                receiver: None,
                                binding: None,
                                source_line: None,
                            });
                            promoted_event = true;
                        }
                    }
                }
                let classified = commands
                    .iter()
                    .map(|command| {
                        let root_context_execute =
                            root_command_context_allowed_on_track(track, depth, command);
                        if root_context_execute
                            && command_matches_track(track, command)
                            && !matches!(command, ExcuteStmt::Raw(_))
                            && validate_command_on_track(fighter, track, command).is_ok()
                        {
                            MoveCommand::Editable(command.clone())
                        } else {
                            MoveCommand::Preserved(command.clone())
                        }
                    })
                    .collect::<Vec<_>>();
                let editable_count = classified
                    .iter()
                    .filter(|command| matches!(command, MoveCommand::Editable(_)))
                    .count();
                if editable_count == commands.len() {
                    NodeKind::Execute { commands }
                } else if editable_count > 0 {
                    baseline.push(if promoted_event {
                        AcmdStmt::Excute(commands.clone())
                    } else {
                        stmt.clone()
                    });
                    NodeKind::MixedExecute {
                        commands: classified,
                    }
                } else {
                    baseline.push(stmt.clone());
                    NodeKind::Preserved {
                        statement: stmt.clone(),
                    }
                }
            }
            AcmdStmt::Bare(command)
                if (bare_command_matches_track(track, command)
                    || execute_context && command_matches_track(track, command))
                    && root_command_context_allowed_on_track(track, depth, command)
                    && !matches!(command.as_ref(), ExcuteStmt::Raw(_))
                    && validate_command_on_track(fighter, track, command).is_ok() =>
            {
                NodeKind::BareCommand {
                    command: command.clone(),
                }
            }
            _ => {
                baseline.push(stmt.clone());
                NodeKind::Preserved {
                    statement: stmt.clone(),
                }
            }
        };
        out.push(MoveNode {
            id: fresh_id(next_id),
            kind,
        });
        index += 1;
    }
    out
}

/// The fallback for an `if`/`else` header no typed condition reads (a dumped address call,
/// a lone `else {`): the header is kept byte-exact and the blocks under it import as usual.
/// Loop headers stay preserved, since a branch cannot show a body that runs more than once,
/// and so do Execute guards, whose context their children's validation depends on. Like
/// every imported condition this needs fighter context.
pub(super) fn source_condition(fighter: Option<&str>, header: &str) -> Option<MoveCondition> {
    fighter?;
    let line = header.trim();
    let branch = line == "else {"
        || ["if ", "if(", "if!", "else if "]
            .iter()
            .any(|prefix| line.starts_with(prefix));
    (branch
        && line.ends_with('{')
        && line.matches('{').count() == 1
        && !line.contains('}')
        && !line.contains("is_excute"))
    .then(|| MoveCondition::Source {
        header: header.to_owned(),
    })
}

/// An `if` source header needs a statement with no attribute, which would also govern the
/// typed blocks under it. An `else` header continues only an `if` already typed — a source
/// branch, or an Execute guard the dump wrote with a separate alternate — since the arm of a
/// preserved `if` must stay with it. Moving either apart fails the compiler's syntax check.
pub(super) fn source_branch_allowed(header: &str, previous: &[MoveNode], clean_if: bool) -> bool {
    if header.trim_start().starts_with("else") {
        match previous.last().map(|node| &node.kind) {
            Some(NodeKind::Branch {
                condition: MoveCondition::Source { .. },
                else_nodes,
                ..
            }) => else_nodes.is_empty(),
            Some(
                NodeKind::Execute { .. }
                | NodeKind::MixedExecute { .. }
                | NodeKind::EffectExecute { .. }
                | NodeKind::MixedEffectExecute { .. },
            ) => true,
            _ => false,
        }
    } else {
        clean_if
    }
}

/// The baseline entry that proves a [`MoveCondition::Source`] header was imported, not
/// authored: the header's own block with its body lifted out into typed nodes.
pub(super) fn source_condition_witness(header: &str) -> AcmdStmt {
    AcmdStmt::RawBlock {
        header: header.to_owned(),
        body: Vec::new(),
    }
}

pub(super) fn import_condition(fighter: Option<&str>, header: &str) -> Option<MoveCondition> {
    // The legacy API has no fighter context and intentionally keeps control flow immutable.
    super::compound_conditions::import(fighter?, header)
}

fn import_condition_for_track(
    fighter: Option<&str>,
    track: Track,
    header: &str,
) -> Option<MoveCondition> {
    let fighter = fighter?;
    let condition = import_condition(Some(fighter), header)?;
    condition_allowed_on_track(track, &condition).then_some(condition)
}

pub(super) fn import_atomic_condition(fighter: &str, header: &str) -> Option<MoveCondition> {
    let header = header.trim();
    if header == "if ItemModule::is_have_item(agent.module_accessor, 0) {"
        && registry::fighter_kind_symbol(fighter).is_some()
    {
        return Some(MoveCondition::ItemHeld);
    }
    match header {
        "if macros::is_excute(agent) {" => return Some(MoveCondition::IsExecute),
        "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND {" => {
            return Some(MoveCondition::Grounded);
        }
        "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_AIR {" => {
            return Some(MoveCondition::Airborne);
        }
        "if MotionModule::is_changing(agent.module_accessor) {" => {
            return Some(MoveCondition::MotionChanging);
        }
        _ => {}
    }

    if let Some(symbol) = header
        .strip_prefix("if macros::IS_EXIST_ARTICLE(agent, *")
        .and_then(|header| header.strip_suffix(") {"))
    {
        validate_article_symbol(fighter, symbol).ok()?;
        return Some(MoveCondition::ArticleExists {
            symbol: symbol.to_owned(),
        });
    }

    if let Some(symbol) = header
        .strip_prefix("if StatusModule::status_kind(agent.module_accessor) == *")
        .and_then(|header| header.strip_suffix(" {"))
    {
        let condition = MoveCondition::InStatus {
            symbol: symbol.to_owned(),
        };
        return atomic_condition_expression(fighter, &condition)
            .is_ok()
            .then_some(condition);
    }
    if let Some(item) = header
        .strip_prefix("if ItemModule::get_have_item_kind(agent.module_accessor, 0) == *")
        .and_then(|header| header.strip_suffix(" {"))
    {
        let condition = MoveCondition::HoldingItemKind {
            item: item.to_owned(),
        };
        return atomic_condition_expression(fighter, &condition)
            .is_ok()
            .then_some(condition);
    }

    if header == "if get_value_float(agent.lua_state_agent, *SO_VAR_FLOAT_LR) < 0.0 {" {
        return Some(MoveCondition::NativeLrNegative);
    }

    for (prefix, expected) in [
        ("if WorkModule::is_flag(agent.module_accessor, *", true),
        ("if !WorkModule::is_flag(agent.module_accessor, *", false),
    ] {
        if let Some(symbol) = header
            .strip_prefix(prefix)
            .and_then(|header| header.strip_suffix(") {"))
        {
            let symbol =
                super::native_work::validate_condition_flag(fighter, &format!("*{symbol}")).ok()?;
            return Some(MoveCondition::WorkFlag { symbol, expected });
        }
    }

    for axis in [super::model::StickAxis::X, super::model::StickAxis::Y] {
        for relative_to_facing in [false, true] {
            if relative_to_facing && axis == super::model::StickAxis::Y {
                continue;
            }
            let read = stick_read(axis, relative_to_facing);
            for op in super::model::NativeComparison::ALL {
                let prefix = format!("if {read} {} ", op.token());
                if let Some(raw) = header
                    .strip_prefix(&prefix)
                    .and_then(|s| s.strip_suffix(" {"))
                {
                    let value = raw.parse::<f32>().ok()?;
                    if !(-1.0..=1.0).contains(&value) {
                        return None;
                    }
                    return Some(MoveCondition::Stick {
                        axis,
                        op,
                        value,
                        relative_to_facing,
                    });
                }
            }
        }
    }

    for (constant, button) in [
        ("CONTROL_PAD_BUTTON_ATTACK", InputButton::Attack),
        ("CONTROL_PAD_BUTTON_SPECIAL", InputButton::Special),
        ("CONTROL_PAD_BUTTON_JUMP", InputButton::Jump),
        ("CONTROL_PAD_BUTTON_GUARD", InputButton::Guard),
        ("CONTROL_PAD_BUTTON_CATCH", InputButton::Grab),
    ] {
        if header
            == format!("if ControlModule::check_button_on(agent.module_accessor, *{constant}) {{")
        {
            return Some(MoveCondition::ButtonHeld { button });
        }
    }

    for (query, getter) in [
        (InflictionQuery::CurrentHitlag, "is_infliction"),
        (InflictionQuery::CurrentStatus, "is_infliction_status"),
    ] {
        for (mask, symbol) in [
            (InflictionMask::Hit, "HIT"),
            (InflictionMask::Shield, "SHIELD"),
        ] {
            let expected = format!(
                "if AttackModule::{getter}(agent.module_accessor, *COLLISION_KIND_MASK_{symbol}) {{"
            );
            if header == expected {
                return Some(MoveCondition::AttackInfliction { query, mask });
            }
        }
    }
    super::character_values::import(header)
        .or_else(|| super::native_conditions::import(fighter, header))
}

pub(super) fn condition_allowed_on_track(track: Track, condition: &MoveCondition) -> bool {
    match condition {
        MoveCondition::ItemHeld => track == Track::Expression,
        MoveCondition::MotionChanging => track == Track::Game,
        MoveCondition::AttackInfliction { .. } => track == Track::Game,
        MoveCondition::NativeLrNegative => {
            matches!(track, Track::Game | Track::Effect | Track::Expression)
        }
        MoveCondition::Not(inner) | MoveCondition::Group(inner) => {
            condition_allowed_on_track(track, inner)
        }
        MoveCondition::And { left, right } | MoveCondition::Or { left, right } => {
            condition_allowed_on_track(track, left) && condition_allowed_on_track(track, right)
        }
        _ => true,
    }
}

fn contains_native_lr_negative(condition: &MoveCondition) -> bool {
    match condition {
        MoveCondition::NativeLrNegative => true,
        MoveCondition::Not(inner) | MoveCondition::Group(inner) => {
            contains_native_lr_negative(inner)
        }
        MoveCondition::And { left, right } | MoveCondition::Or { left, right } => {
            contains_native_lr_negative(left) || contains_native_lr_negative(right)
        }
        _ => false,
    }
}

/// Validate and lower every track atomically. Any issue rejects the whole graph.
pub fn compile(
    fighter: &str,
    move_name: &str,
    graph: &MoveGraph,
) -> Result<Vec<(Track, AcmdScript)>, Vec<NodeError>> {
    let mut errors = super::flow::validate_layout(graph);
    let mut output = Vec::with_capacity(graph.tracks.len());
    let mut global_ids = HashSet::new();
    let mut seen_tracks = Vec::new();
    let mut expanded = 0u64;
    if graph.version != SCHEMA_VERSION {
        errors.push(error(
            fighter,
            "graph.version",
            format!(
                "unsupported schema {}; expected {SCHEMA_VERSION}",
                graph.version
            ),
        ));
    }
    if syn::parse_str::<syn::Ident>(fighter).is_err() {
        errors.push(error(fighter, "fighter", "must be a Rust identifier"));
    }
    if syn::parse_str::<syn::Ident>(move_name).is_err() {
        errors.push(error(fighter, "move_name", "must be a Rust identifier"));
    }
    if graph.tracks.len() > 4 {
        errors.push(error(
            fighter,
            "tracks",
            "at most one of each supported track is allowed",
        ));
        return Err(errors);
    }

    for (track_index, track) in graph.tracks.iter().enumerate() {
        let track_path = format!("tracks[{track_index}]");
        if seen_tracks.contains(&track.track) {
            errors.push(error(fighter, &track_path, "duplicate track"));
        } else {
            seen_tracks.push(track.track);
        }
        let baseline_values = track
            .baseline
            .iter()
            .map(json_value)
            .collect::<Result<Vec<_>, _>>();
        let mut baseline_remaining: HashMap<String, usize> = HashMap::new();
        match baseline_values {
            Ok(values) => {
                for value in values {
                    *baseline_remaining.entry(value.to_string()).or_default() += 1;
                }
            }
            Err(reason) => errors.push(error(fighter, &format!("{track_path}.baseline"), reason)),
        }
        if track.track != Track::Effect && !track.effect_execute_baseline.is_empty() {
            errors.push(error(
                fighter,
                &format!("{track_path}.effect_execute_baseline"),
                "mixed effect witnesses are only valid on the Effect track",
            ));
        }
        if track.effect_execute_baseline.len() > MAX_NODES {
            errors.push(error(
                fighter,
                &format!("{track_path}.effect_execute_baseline"),
                format!("mixed effect witness count exceeds {MAX_NODES}"),
            ));
        } else {
            for (witness_index, commands) in track.effect_execute_baseline.iter().enumerate() {
                if commands.is_empty() || commands.len() > MAX_COMMANDS_PER_EXECUTE {
                    errors.push(error(
                        fighter,
                        &format!("{track_path}.effect_execute_baseline[{witness_index}]"),
                        format!(
                            "mixed effect witness must contain 1..={MAX_COMMANDS_PER_EXECUTE} commands"
                        ),
                    ));
                    continue;
                }
                match effect_execute_witness_key(commands) {
                    Ok(key) => *baseline_remaining.entry(key).or_default() += 1,
                    Err(reason) => errors.push(error(
                        fighter,
                        &format!("{track_path}.effect_execute_baseline[{witness_index}]"),
                        reason,
                    )),
                }
            }
        }
        let nodes = compile_nodes(
            fighter,
            track.track,
            &track_path,
            &track.nodes,
            &track.baseline,
            0,
            &mut global_ids,
            &mut baseline_remaining,
            &mut expanded,
            &mut errors,
            false,
        );
        if expanded > MAX_EXPANDED_NODES {
            errors.push(error(
                fighter,
                &track_path,
                format!("expanded node budget exceeded ({MAX_EXPANDED_NODES})"),
            ));
        }
        if !baseline_remaining.is_empty() {
            errors.push(error(
                fighter,
                &format!("{track_path}.baseline"),
                "baseline source or mixed-effect witness was removed or replaced; imported source is immutable",
            ));
        }
        let script = AcmdScript { stmts: nodes };
        if let Some(reason) = script.preview_budget_error() {
            errors.push(error(fighter, &track_path, reason));
        } else {
            let emitted = preview(track.track, &script, move_name);
            if let Err(parse_error) = syn::parse_file(&emitted) {
                errors.push(error(
                    fighter,
                    &track_path,
                    format!("generated ACMD failed Rust parsing: {parse_error}"),
                ));
            }
        }
        output.push((track.track, script));
    }
    if global_ids.len() > MAX_NODES {
        errors.push(error(
            fighter,
            "tracks",
            format!("graph node count exceeds {MAX_NODES}"),
        ));
    }

    if errors.is_empty() {
        Ok(output)
    } else {
        Err(errors)
    }
}

/// A statement that itself waits for the motion to loop.
fn waits_for_motion_loop(stmt: &AcmdStmt) -> bool {
    match stmt {
        AcmdStmt::WaitLoopClear | AcmdStmt::NativeMotionLoopSync => true,
        AcmdStmt::Loop { motion_loop, .. } => motion_loop.is_loop(),
        _ => false,
    }
}

/// Where each motion loop may stand: `wait_loop_clear` only where vanilla calls it, and
/// `wait_loop_sync_mot` on every track (presentation scripts, plus 6 Game scripts).
fn motion_loop_track(motion_loop: crate::data::MotionLoop, track: Track) -> bool {
    match motion_loop {
        crate::data::MotionLoop::No => false,
        crate::data::MotionLoop::Clear => wait_loop_clear_track(track),
        crate::data::MotionLoop::SyncMotion => true,
    }
}

/// Tracks whose vanilla scripts call `wait_loop_clear` (43 Game and 21 Expression sites).
fn wait_loop_clear_track(track: Track) -> bool {
    matches!(track, Track::Game | Track::Expression)
}

#[allow(clippy::too_many_arguments)]
fn compile_nodes(
    fighter: &str,
    track: Track,
    path: &str,
    nodes: &[MoveNode],
    baseline_source: &[AcmdStmt],
    depth: usize,
    ids: &mut HashSet<u32>,
    baseline_remaining: &mut HashMap<String, usize>,
    expanded: &mut u64,
    errors: &mut Vec<NodeError>,
    execute_context: bool,
) -> Vec<AcmdStmt> {
    if depth > MAX_NESTING {
        errors.push(error(
            fighter,
            path,
            format!("nesting exceeds {MAX_NESTING}"),
        ));
        return Vec::new();
    }
    if nodes.len() > MAX_NODES {
        errors.push(error(
            fighter,
            path,
            format!("node count exceeds {MAX_NODES}"),
        ));
        return Vec::new();
    }
    let mut out = Vec::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        let node_path = format!("{path}.nodes[{index}]#{}", node.id);
        if !ids.insert(node.id) {
            errors.push(error(fighter, &node_path, "duplicate node id"));
        }
        *expanded = expanded.saturating_add(1);
        if *expanded > MAX_EXPANDED_NODES {
            errors.push(error(
                fighter,
                &node_path,
                "expanded instruction budget exceeded",
            ));
            continue;
        }
        match &node.kind {
            NodeKind::Frame { frame } => {
                if !frame.is_finite() || *frame < 0.0 {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "frame must be finite and nonnegative",
                    ));
                } else {
                    out.push(AcmdStmt::Frame(*frame));
                }
            }
            NodeKind::NativeExecuteFrame { frame } => {
                if depth != 0 || track == Track::Sound {
                    errors.push(error(fighter, &node_path,
                        "native execute frame requires a direct root Game, Effect or Expression timing block"));
                } else if !valid_native_execute_frame(*frame) {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "native execute frame must be a finite number from 0 through 1000000",
                    ));
                } else {
                    out.push(AcmdStmt::NativeExecuteFrame(*frame));
                }
            }
            NodeKind::NativeWaitLoopClear => {
                if depth != 0 || !wait_loop_clear_track(track) {
                    errors.push(error(fighter, &node_path,
                        "wait for motion loop requires a direct root Game or Expression timing block"));
                } else {
                    out.push(AcmdStmt::WaitLoopClear);
                }
            }
            NodeKind::NativeMotionLoopSync => {
                if depth != 0 || track == Track::Game {
                    errors.push(error(fighter, &node_path,
                        "native motion-loop sync requires a direct root Effect, Sound or Expression timing block"));
                } else {
                    out.push(AcmdStmt::NativeMotionLoopSync);
                }
            }
            NodeKind::Wait { frames } => {
                if !frames.is_finite() || *frames < 0.0 {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "wait must be finite and nonnegative",
                    ));
                } else {
                    out.push(AcmdStmt::Wait(*frames));
                }
            }
            NodeKind::MotionRate { rate } => {
                if track != Track::Game {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "motion rate is only valid on the Game track",
                    ));
                } else if !valid_motion_rate(*rate) {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "motion rate must be finite and greater than zero",
                    ));
                } else {
                    *expanded = expanded.saturating_add(1);
                    out.push(AcmdStmt::MotionRate(*rate));
                }
            }
            NodeKind::BareCommand { command } => {
                *expanded = expanded.saturating_add(1);
                if matches!(command.as_ref(), ExcuteStmt::Raw(_)) {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "Raw commands are immutable imported baseline source and cannot be authored",
                    ));
                } else if !(bare_command_matches_track(track, command)
                    || execute_context && command_matches_track(track, command))
                {
                    errors.push(error(
                        fighter,
                        &node_path,
                        format!("bare command is not supported on the {track:?} track"),
                    ));
                } else if !root_command_context_allowed_on_track(track, depth, command) {
                    errors.push(error(
                        fighter,
                        &node_path,
                        root_command_context_error_on_track(track, command),
                    ));
                } else if let Err(reason) = validate_command_on_track(Some(fighter), track, command)
                {
                    errors.push(error(fighter, &node_path, reason));
                } else {
                    out.push(AcmdStmt::Bare(command.clone()));
                }
            }
            NodeKind::Execute { commands } => {
                if commands.is_empty() {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "execute block must contain a command",
                    ));
                }
                if commands.len() > MAX_COMMANDS_PER_EXECUTE {
                    errors.push(error(
                        fighter,
                        &node_path,
                        format!("execute block exceeds {MAX_COMMANDS_PER_EXECUTE} commands"),
                    ));
                }
                *expanded =
                    expanded.saturating_add(commands.len().min(MAX_COMMANDS_PER_EXECUTE) as u64);
                for (command_index, command) in
                    commands.iter().take(MAX_COMMANDS_PER_EXECUTE).enumerate()
                {
                    if matches!(command, ExcuteStmt::Raw(_)) {
                        errors.push(error(
                            fighter,
                            &format!("{node_path}.commands[{command_index}]"),
                            "Raw commands are immutable imported baseline source and cannot be authored",
                        ));
                    } else if !command_matches_track(track, command) {
                        errors.push(error(
                            fighter,
                            &format!("{node_path}.commands[{command_index}]"),
                            format!("command is not valid on the {track:?} track"),
                        ));
                    } else if !root_command_context_allowed_on_track(track, depth, command) {
                        errors.push(error(
                            fighter,
                            &format!("{node_path}.commands[{command_index}]"),
                            root_command_context_error_on_track(track, command),
                        ));
                    } else if let Err(reason) =
                        validate_command_on_track(Some(fighter), track, command)
                    {
                        errors.push(error(
                            fighter,
                            &format!("{node_path}.commands[{command_index}]"),
                            reason,
                        ));
                    }
                }
                if commands.len() <= MAX_COMMANDS_PER_EXECUTE {
                    out.push(AcmdStmt::Excute(commands.clone()));
                }
            }
            NodeKind::MixedExecute { commands } => {
                let command_path =
                    |command_index: usize| format!("{node_path}.commands[{command_index}]");
                if commands.is_empty() {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "mixed execute block must contain a command",
                    ));
                }
                if commands.len() > MAX_COMMANDS_PER_EXECUTE {
                    errors.push(error(
                        fighter,
                        &node_path,
                        format!("mixed execute block exceeds {MAX_COMMANDS_PER_EXECUTE} commands"),
                    ));
                }
                *expanded =
                    expanded.saturating_add(commands.len().min(MAX_COMMANDS_PER_EXECUTE) as u64);
                let mut editable_count = 0usize;
                let mut preserved_count = 0usize;
                let mut generated = Vec::with_capacity(commands.len());
                let mut commands_valid = commands.len() <= MAX_COMMANDS_PER_EXECUTE;
                for (command_index, command) in
                    commands.iter().take(MAX_COMMANDS_PER_EXECUTE).enumerate()
                {
                    let source_command = match command {
                        MoveCommand::Editable(command) => {
                            editable_count += 1;
                            if matches!(command, ExcuteStmt::Raw(_)) {
                                errors.push(error(
                                    fighter,
                                    &command_path(command_index),
                                    "Raw commands cannot be made editable",
                                ));
                                commands_valid = false;
                            } else if !command_matches_track(track, command) {
                                errors.push(error(
                                    fighter,
                                    &command_path(command_index),
                                    format!("command is not valid on the {track:?} track"),
                                ));
                                commands_valid = false;
                            } else if !root_command_context_allowed_on_track(track, depth, command)
                            {
                                errors.push(error(
                                    fighter,
                                    &command_path(command_index),
                                    root_command_context_error_on_track(track, command),
                                ));
                                commands_valid = false;
                            } else if let Err(reason) =
                                validate_command_on_track(Some(fighter), track, command)
                            {
                                errors.push(error(fighter, &command_path(command_index), reason));
                                commands_valid = false;
                            }
                            command
                        }
                        MoveCommand::Preserved(command) => {
                            preserved_count += 1;
                            command
                        }
                    };
                    generated.push(source_command.clone());
                }
                if editable_count == 0 || preserved_count == 0 {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "mixed execute must contain both editable and preserved commands",
                    ));
                    commands_valid = false;
                }

                let source_key = baseline_source.iter().find_map(|statement| {
                    let AcmdStmt::Excute(original) = statement else {
                        return None;
                    };
                    if !mixed_matches_baseline(commands, original) {
                        return None;
                    }
                    let key = json_value(statement).ok()?.to_string();
                    baseline_remaining
                        .get(&key)
                        .is_some_and(|remaining| *remaining > 0)
                        .then_some(key)
                });
                if let Some(key) = source_key {
                    decrement_remaining(baseline_remaining, &key);
                } else {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "mixed execute does not match an unused imported baseline execute wrapper",
                    ));
                    commands_valid = false;
                }
                if commands_valid {
                    out.push(AcmdStmt::Excute(generated));
                }
            }
            NodeKind::EffectExecute { commands } => {
                if track != Track::Effect {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "effect commands are only valid on the Effect track",
                    ));
                }
                if commands.is_empty() {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "effect execute block must contain a command",
                    ));
                }
                if commands.len() > MAX_COMMANDS_PER_EXECUTE {
                    errors.push(error(
                        fighter,
                        &node_path,
                        format!("effect execute block exceeds {MAX_COMMANDS_PER_EXECUTE} commands"),
                    ));
                }
                *expanded =
                    expanded.saturating_add(commands.len().min(MAX_COMMANDS_PER_EXECUTE) as u64);
                let mut generated = Vec::new();
                for (command_index, command) in
                    commands.iter().take(MAX_COMMANDS_PER_EXECUTE).enumerate()
                {
                    if matches!(command, crate::data::EffectMacro::EffectFollowWork { .. }) {
                        errors.push(error(
                            fighter,
                            &format!("{node_path}.commands[{command_index}]"),
                            "EFFECT_FOLLOW_WORK requires its imported mixed-effect source witness",
                        ));
                        continue;
                    }
                    match crate::move_nodes::effects::command_lines_for_fighter(
                        Some(fighter),
                        command,
                    ) {
                        Ok(lines) => generated.extend(lines.into_iter().map(ExcuteStmt::Raw)),
                        Err(reason) => errors.push(error(
                            fighter,
                            &format!("{node_path}.commands[{command_index}]"),
                            reason,
                        )),
                    }
                }
                if track == Track::Effect
                    && commands.len() <= MAX_COMMANDS_PER_EXECUTE
                    && !generated.is_empty()
                {
                    out.push(AcmdStmt::Excute(generated));
                }
            }
            NodeKind::MixedEffectExecute {
                commands,
                source_commands,
            } => {
                let command_path =
                    |command_index: usize| format!("{node_path}.commands[{command_index}]");
                if track != Track::Effect {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "effect commands are only valid on the Effect track",
                    ));
                }
                if commands.is_empty() || commands.len() != source_commands.len() {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "mixed effect command slots must match a non-empty source wrapper",
                    ));
                }
                if commands.len() > MAX_COMMANDS_PER_EXECUTE
                    || source_commands.len() > MAX_COMMANDS_PER_EXECUTE
                {
                    errors.push(error(
                        fighter,
                        &node_path,
                        format!("mixed effect execute block and source witness exceed {MAX_COMMANDS_PER_EXECUTE} commands"),
                    ));
                    continue;
                }
                *expanded =
                    expanded.saturating_add(commands.len().min(MAX_COMMANDS_PER_EXECUTE) as u64);

                let mut editable_count = 0usize;
                let mut preserved_count = 0usize;
                let mut generated = Vec::new();
                let mut commands_valid = commands.len() == source_commands.len()
                    && !commands.is_empty()
                    && commands.len() <= MAX_COMMANDS_PER_EXECUTE;
                for (command_index, (slot, original)) in commands
                    .iter()
                    .zip(source_commands)
                    .take(MAX_COMMANDS_PER_EXECUTE)
                    .enumerate()
                {
                    match slot {
                        EffectCommandSlot::Editable(command) => {
                            editable_count += 1;
                            if crate::move_nodes::effects::command_lines_for_fighter(
                                Some(fighter),
                                original,
                            )
                            .is_err()
                                || !effect_command_family_matches(original, command)
                            {
                                errors.push(error(
                                    fighter,
                                    &command_path(command_index),
                                    "editable effect command must retain its imported command family",
                                ));
                                commands_valid = false;
                            }
                            match crate::move_nodes::effects::command_lines_for_fighter(
                                Some(fighter),
                                command,
                            ) {
                                Ok(lines) => {
                                    generated.extend(lines.into_iter().map(ExcuteStmt::Raw));
                                }
                                Err(reason) => {
                                    errors.push(error(
                                        fighter,
                                        &command_path(command_index),
                                        reason,
                                    ));
                                    commands_valid = false;
                                }
                            }
                        }
                        EffectCommandSlot::Preserved(command) => {
                            preserved_count += 1;
                            let same_command = matches!(
                                (json_value(command), json_value(original)),
                                (Ok(current), Ok(original)) if current == original
                            );
                            if !same_command
                                || crate::move_nodes::effects::command_lines_for_fighter(
                                    Some(fighter),
                                    original,
                                )
                                .is_ok()
                            {
                                errors.push(error(
                                    fighter,
                                    &command_path(command_index),
                                    "preserved effect command must remain unchanged in its imported slot",
                                ));
                                commands_valid = false;
                            }
                            generated.push(ExcuteStmt::Raw(
                                crate::acmd::preview_effect_macro(command).trim().to_owned(),
                            ));
                        }
                    }
                }
                let has_follow_work = source_commands.iter().any(|command| {
                    matches!(command, crate::data::EffectMacro::EffectFollowWork { .. })
                });
                if editable_count == 0 || (preserved_count == 0 && !has_follow_work) {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "mixed effect execute must contain preserved source or an anchored EFFECT_FOLLOW_WORK command",
                    ));
                    commands_valid = false;
                }

                let source_wrapper = AcmdStmt::RawBlock {
                    header: "if macros::is_excute(agent) {".into(),
                    body: source_commands
                        .iter()
                        .map(|command| {
                            AcmdStmt::Raw(
                                crate::acmd::preview_effect_macro(command).trim().to_owned(),
                            )
                        })
                        .collect(),
                };
                let source_matches = crate::move_nodes::effects::effect_wrapper_matches_source(
                    source_commands,
                    &source_wrapper,
                );
                let wrapper_key = source_matches
                    .then(|| json_value(&source_wrapper).ok())
                    .flatten()
                    .map(|value| value.to_string())
                    .filter(|key| {
                        baseline_remaining
                            .get(key)
                            .is_some_and(|remaining| *remaining > 0)
                    });
                let witness_key = source_matches
                    .then(|| effect_execute_witness_key(source_commands).ok())
                    .flatten()
                    .filter(|key| {
                        baseline_remaining
                            .get(key)
                            .is_some_and(|remaining| *remaining > 0)
                    });
                if let (Some(wrapper_key), Some(witness_key)) = (wrapper_key, witness_key) {
                    decrement_remaining(baseline_remaining, &wrapper_key);
                    decrement_remaining(baseline_remaining, &witness_key);
                } else {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "mixed effect execute does not match an unused imported baseline wrapper",
                    ));
                    commands_valid = false;
                }
                if commands_valid && track == Track::Effect && !generated.is_empty() {
                    out.push(AcmdStmt::Excute(generated));
                }
            }
            NodeKind::Repeat {
                count,
                body,
                motion_loop,
            } if motion_loop.is_loop() => {
                if *count != 1 {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "an every-motion-loop repeat runs once per loop; its count must be 1",
                    ));
                    continue;
                }
                if depth != 0 || !motion_loop_track(*motion_loop, track) {
                    errors.push(error(fighter, &node_path, if *motion_loop == crate::data::MotionLoop::Clear {
                        "every-motion-loop repeat (loop clear) requires a direct root Game or Expression timing block"
                    } else {
                        "every-motion-loop repeat requires a direct root timing block"
                    }));
                    continue;
                }
                if index + 1 != nodes.len() {
                    errors.push(error(fighter, &node_path,
                        "every-motion-loop repeat never ends, so it must be the last block of its track"));
                    continue;
                }
                // Compiled as the root it replaces, so root-only blocks stay valid inside it.
                let body = compile_nodes(
                    fighter,
                    track,
                    &node_path,
                    body,
                    baseline_source,
                    depth,
                    ids,
                    baseline_remaining,
                    expanded,
                    errors,
                    execute_context,
                );
                if body.iter().any(waits_for_motion_loop) {
                    errors.push(error(fighter, &node_path,
                        "an every-motion-loop repeat already waits for the motion loop; remove the inner wait or loop"));
                    continue;
                }
                out.push(AcmdStmt::Loop {
                    count: 1,
                    body,
                    motion_loop: *motion_loop,
                });
            }
            NodeKind::Repeat { count, body, .. } => {
                if *count == 0 || *count > MAX_REPEAT {
                    errors.push(error(
                        fighter,
                        &node_path,
                        format!("repeat count must be between 1 and {MAX_REPEAT}"),
                    ));
                    continue;
                }
                if depth >= MAX_NESTING {
                    errors.push(error(
                        fighter,
                        &node_path,
                        format!("nesting exceeds {MAX_NESTING}"),
                    ));
                    continue;
                }
                let before = *expanded;
                let compiled_body = compile_nodes(
                    fighter,
                    track,
                    &node_path,
                    body,
                    baseline_source,
                    depth + 1,
                    ids,
                    baseline_remaining,
                    expanded,
                    errors,
                    execute_context,
                );
                let body_size = expanded.saturating_sub(before);
                *expanded =
                    expanded.saturating_add(body_size.saturating_mul(u64::from(*count - 1)));
                if *expanded > MAX_EXPANDED_NODES {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "expanded instruction budget exceeded",
                    ));
                }
                out.push(AcmdStmt::Loop {
                    motion_loop: crate::data::MotionLoop::No,
                    count: *count as usize,
                    body: compiled_body,
                });
            }
            NodeKind::Branch {
                condition,
                then_nodes,
                else_nodes,
            } => {
                if !condition_allowed_on_track(track, condition) {
                    let reason = if track == Track::Sound && contains_native_lr_negative(condition)
                    {
                        "native LR-sign source conditions are supported only on Game, Effect, and Expression tracks"
                    } else if contains_attack_infliction(condition) {
                        "AttackModule infliction conditions are supported only on the Game track"
                    } else if contains_item_held(condition) {
                        "ItemModule held-item conditions are supported only on the Expression track"
                    } else {
                        "MotionModule::is_changing conditions are supported only on the Game track"
                    };
                    errors.push(error(fighter, &node_path, reason));
                    continue;
                }
                let header = match branch_condition(fighter, condition) {
                    Ok(header) => header,
                    Err(reason) => {
                        errors.push(error(fighter, &node_path, reason));
                        continue;
                    }
                };
                if let MoveCondition::Source { header } = condition {
                    if !take_baseline(baseline_remaining, &source_condition_witness(header)) {
                        errors.push(error(fighter, &node_path,
                            "source condition does not match an unused imported header; it cannot be authored"));
                        continue;
                    }
                }
                if depth >= MAX_NESTING {
                    errors.push(error(
                        fighter,
                        &node_path,
                        format!("nesting exceeds {MAX_NESTING}"),
                    ));
                    continue;
                }
                let body = compile_nodes(
                    fighter,
                    track,
                    &node_path,
                    then_nodes,
                    baseline_source,
                    depth + 1,
                    ids,
                    baseline_remaining,
                    expanded,
                    errors,
                    execute_context || matches!(condition, MoveCondition::IsExecute),
                );
                let else_body = compile_nodes(
                    fighter,
                    track,
                    &node_path,
                    else_nodes,
                    baseline_source,
                    depth + 1,
                    ids,
                    baseline_remaining,
                    expanded,
                    errors,
                    execute_context,
                );
                if *expanded > MAX_EXPANDED_NODES {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "expanded instruction budget exceeded",
                    ));
                }
                out.push(AcmdStmt::RawBlock { header, body });
                if !else_nodes.is_empty() {
                    // `emit_stmts` closes every RawBlock itself. Keep this as the next
                    // sibling so Rust parses it as the preceding `if`'s else arm.
                    out.push(AcmdStmt::RawBlock {
                        header: "else {".into(),
                        body: else_body,
                    });
                }
            }
            NodeKind::Preserved { statement } => {
                if take_baseline(baseline_remaining, statement) {
                    out.push(statement.clone());
                } else {
                    errors.push(error(
                        fighter,
                        &node_path,
                        "preserved statement does not exactly match an unused imported baseline statement",
                    ));
                }
            }
        }
    }
    out
}

/// Consume one unused imported baseline statement equal to `statement`.
fn take_baseline(baseline_remaining: &mut HashMap<String, usize>, statement: &AcmdStmt) -> bool {
    let Ok(value) = json_value(statement) else {
        return false;
    };
    match baseline_remaining.get_mut(&value.to_string()) {
        Some(remaining) if *remaining > 1 => *remaining -= 1,
        Some(_) => {
            baseline_remaining.remove(&value.to_string());
        }
        None => return false,
    }
    true
}

pub(super) fn contains_execute_condition(condition: &MoveCondition) -> bool {
    match condition {
        MoveCondition::IsExecute => true,
        MoveCondition::Not(inner) | MoveCondition::Group(inner) => {
            contains_execute_condition(inner)
        }
        MoveCondition::And { left, right } | MoveCondition::Or { left, right } => {
            contains_execute_condition(left) || contains_execute_condition(right)
        }
        _ => false,
    }
}

fn contains_attack_infliction(condition: &MoveCondition) -> bool {
    match condition {
        MoveCondition::AttackInfliction { .. } => true,
        MoveCondition::Not(inner) | MoveCondition::Group(inner) => {
            contains_attack_infliction(inner)
        }
        MoveCondition::And { left, right } | MoveCondition::Or { left, right } => {
            contains_attack_infliction(left) || contains_attack_infliction(right)
        }
        _ => false,
    }
}

fn contains_item_held(condition: &MoveCondition) -> bool {
    match condition {
        MoveCondition::ItemHeld => true,
        MoveCondition::Not(inner) | MoveCondition::Group(inner) => contains_item_held(inner),
        MoveCondition::And { left, right } | MoveCondition::Or { left, right } => {
            contains_item_held(left) || contains_item_held(right)
        }
        _ => false,
    }
}

fn branch_condition(fighter: &str, condition: &MoveCondition) -> Result<String, String> {
    match condition {
        MoveCondition::Source { header } => Ok(header.clone()),
        _ => super::compound_conditions::header(fighter, condition),
    }
}

pub(super) fn atomic_condition_expression(
    fighter: &str,
    condition: &MoveCondition,
) -> Result<String, String> {
    Ok(match condition {
        MoveCondition::Source { .. } => {
            return Err("an imported source header is a whole branch and cannot be combined".into())
        }
        MoveCondition::CharacterValue {
            chart_id,
            var,
            op,
            value,
        } => super::character_values::expression(chart_id, var, *op, *value)?,
        MoveCondition::Grounded => {
            "StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND".into()
        }
        MoveCondition::Airborne => {
            "StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_AIR".into()
        }
        MoveCondition::IsExecute => "macros::is_excute(agent)".into(),
        MoveCondition::MotionChanging => "MotionModule::is_changing(agent.module_accessor)".into(),
        MoveCondition::NativeLrNegative => {
            "get_value_float(agent.lua_state_agent, *SO_VAR_FLOAT_LR) < 0.0".into()
        }
        MoveCondition::ItemHeld => {
            if registry::fighter_kind_symbol(fighter).is_none() {
                return Err("held-item condition requires a known concrete fighter".into());
            }
            "ItemModule::is_have_item(agent.module_accessor, 0)".into()
        }
        MoveCondition::ArticleExists { symbol } => {
            validate_article_symbol(fighter, symbol)?;
            format!("macros::IS_EXIST_ARTICLE(agent, *{symbol})")
        }
        MoveCondition::InStatus { symbol } => {
            registry::resolve(fighter, symbol, registry::IdentifierKind::FighterStatus)
                .map_err(|error| error.to_string())?;
            format!("StatusModule::status_kind(agent.module_accessor) == *{symbol}")
        }
        MoveCondition::HoldingItemKind { item } => {
            if registry::fighter_kind_symbol(fighter).is_none() || item == "ITEM_KIND_NONE" {
                return Err("held-item kind requires a known fighter and a real item".into());
            }
            registry::resolve(fighter, item, registry::IdentifierKind::Item)
                .map_err(|error| error.to_string())?;
            format!("ItemModule::get_have_item_kind(agent.module_accessor, 0) == *{item}")
        }
        MoveCondition::WorkFlag { symbol, expected } => {
            let symbol =
                super::native_work::validate_condition_flag(fighter, &format!("*{symbol}"))?;
            let flag_check = format!("WorkModule::is_flag(agent.module_accessor, *{})", symbol);
            let test = if *expected {
                flag_check
            } else {
                format!("!{flag_check}")
            };
            test
        }
        MoveCondition::Stick {
            axis,
            op,
            value,
            relative_to_facing,
        } => {
            if !(-1.0..=1.0).contains(value)
                || (*relative_to_facing && *axis == super::model::StickAxis::Y)
            {
                return Err("stick condition requires a finite threshold in -1..1 and facing applies only to X".into());
            }
            format!(
                "{} {} {}",
                stick_read(*axis, *relative_to_facing),
                op.token(),
                crate::acmd::num(*value)
            )
        }
        MoveCondition::ButtonHeld { button } => {
            let constant = match button {
                InputButton::Attack => "CONTROL_PAD_BUTTON_ATTACK",
                InputButton::Special => "CONTROL_PAD_BUTTON_SPECIAL",
                InputButton::Jump => "CONTROL_PAD_BUTTON_JUMP",
                InputButton::Guard => "CONTROL_PAD_BUTTON_GUARD",
                InputButton::Grab => "CONTROL_PAD_BUTTON_CATCH",
            };
            format!("ControlModule::check_button_on(agent.module_accessor, *{constant})")
        }
        MoveCondition::AttackInfliction { query, mask } => {
            let getter = match query {
                InflictionQuery::CurrentHitlag => "is_infliction",
                InflictionQuery::CurrentStatus => "is_infliction_status",
            };
            let symbol = match mask {
                InflictionMask::Hit => "HIT",
                InflictionMask::Shield => "SHIELD",
            };
            format!("AttackModule::{getter}(agent.module_accessor, *COLLISION_KIND_MASK_{symbol})")
        }
        MoveCondition::WorkInt { .. } | MoveCondition::WorkFloat { .. } => {
            let header = super::native_conditions::header(fighter, condition)?;
            header
                .strip_prefix("if ")
                .and_then(|header| header.strip_suffix(" {"))
                .ok_or_else(|| "native numeric comparison emitted an invalid header".to_owned())?
                .to_owned()
        }
        MoveCondition::Not(_)
        | MoveCondition::And { .. }
        | MoveCondition::Or { .. }
        | MoveCondition::Group(_) => return Err("expected an atomic native condition".into()),
    })
}

fn root_command_context_error(command: &ExcuteStmt) -> &'static str {
    if matches!(command, ExcuteStmt::StartCutIn) {
        return "cut-in start is limited to a direct root Game execute block";
    }
    if matches!(command, ExcuteStmt::CaptureCutCommon) {
        return "default catch setup is limited to a direct root Game top-level helper";
    }
    if matches!(command, ExcuteStmt::CameraZoomOut) {
        return "camera zoom out is limited to a direct root Game execute block";
    }
    if matches!(command, ExcuteStmt::StartInfoFlashEye) {
        return "eye flash is limited to a direct root Game or Expression execute block";
    }
    if matches!(command, ExcuteStmt::ShootItemBullet) {
        return "item firing is limited to a direct root Game execute block";
    }
    if matches!(
        command,
        ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::ChangeStatus { .. })
    ) {
        return "article state editing is limited to a direct root execute block";
    }
    if matches!(
        command,
        ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::GenerateEnabled { .. })
    ) {
        "enabled article generation is limited to a direct root execute block"
    } else {
        "visibility editing is limited to a direct root execute block"
    }
}

fn root_command_context_error_on_track(track: Track, command: &ExcuteStmt) -> &'static str {
    if track == Track::Expression
        && matches!(
            command,
            ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::ChangeMotion { .. })
        )
    {
        return "article motion is limited to a direct root Expression execute block";
    }
    if track == Track::Expression
        && matches!(
            command,
            ExcuteStmt::AttackModuleSetAttackReferenceJointId { .. }
        )
    {
        return "attack reference joint is limited to a direct root Expression execute block";
    }
    root_command_context_error(command)
}

fn root_command_context_allowed(depth: usize, command: &ExcuteStmt) -> bool {
    match command {
        ExcuteStmt::ShootItemBullet
        | ExcuteStmt::StartInfoFlashEye
        | ExcuteStmt::CameraZoomOut
        | ExcuteStmt::StartCutIn
        | ExcuteStmt::CaptureCutCommon => depth == 0,
        ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::ChangeStatus {
            ..
        }) => depth == 0,
        ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::GenerateEnabled {
            ..
        }) => depth == 0,
        ExcuteStmt::VisibilitySetWhole { .. } | ExcuteStmt::VisibilitySetDefaultInt64 { .. } => {
            depth == 0
        }
        ExcuteStmt::VisibilitySetInt64 { group, status } => {
            depth == 0 || crate::data::body_visibility_catalog::is_legacy_pair(group, status)
        }
        _ => true,
    }
}

fn root_command_context_allowed_on_track(track: Track, depth: usize, command: &ExcuteStmt) -> bool {
    root_command_context_allowed(depth, command)
        && !(track == Track::Expression
            && depth != 0
            && matches!(
                command,
                ExcuteStmt::AttackModuleSetAttackReferenceJointId { .. }
                    | ExcuteStmt::ArticleLifecycle(
                        crate::data::ArticleLifecycleCall::ChangeMotion { .. }
                    )
            ))
}

fn validate_command_on_track(
    fighter: Option<&str>,
    track: Track,
    command: &ExcuteStmt,
) -> Result<(), String> {
    if let (
        Track::Expression,
        ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::ChangeMotion {
            symbol,
            motion,
        }),
    ) = (track, command)
    {
        let fighter = fighter.ok_or_else(|| {
            "article motion editing requires a fighter registry binding".to_owned()
        })?;
        validate_article_symbol(fighter, symbol)?;
        if !crate::data::measured_expression_article_motions(symbol).contains(&motion.as_str()) {
            return Err(format!(
                "Expression article motion {motion:?} is not measured for {fighter}: {symbol}"
            ));
        }
        return Ok(());
    }
    validate_command(fighter, command)
}

fn validate_command(fighter: Option<&str>, command: &ExcuteStmt) -> Result<(), String> {
    if let ExcuteStmt::NativeCall(call) = command {
        return call.validate_for_fighter(fighter);
    }
    if let ExcuteStmt::Effect(command) = command {
        if matches!(
            command.as_ref(),
            crate::data::EffectMacro::EffectFollowWork { .. }
        ) {
            return Err("EFFECT_FOLLOW_WORK requires its imported mixed Execute witness".into());
        }
        return super::effects::command_lines_for_fighter(fighter, command).map(|_| ());
    }
    match command {
        ExcuteStmt::CheckFinishCamera(call) => {
            let fighter = fighter.ok_or_else(|| {
                "CHECK_FINISH_CAMERA editing requires its measured donor fighter".to_owned()
            })?;
            if !call.is_measured_for(fighter) {
                return Err("CHECK_FINISH_CAMERA tuple is not measured for this fighter".into());
            }
        }
        ExcuteStmt::CheckFinalStartCamera(call) => {
            let fighter = fighter.ok_or_else(|| {
                "CHECK_VALID_FINAL_START_CAMERA editing requires its measured donor fighter"
                    .to_owned()
            })?;
            if !call.is_measured_for(fighter) {
                return Err(
                    "CHECK_VALID_FINAL_START_CAMERA tuple is not measured for this fighter".into(),
                );
            }
        }
        ExcuteStmt::FinalFearFace(call) => {
            let fighter = fighter.ok_or_else(|| {
                "FT_SET_FINAL_FEAR_FACE editing requires its measured donor fighter".to_owned()
            })?;
            if !call.is_measured_for(fighter) {
                return Err("FT_SET_FINAL_FEAR_FACE token is not measured for this fighter".into());
            }
        }
        ExcuteStmt::WorkFlag(call) => {
            let fighter = fighter.ok_or_else(|| {
                "work flag editing requires a fighter registry binding".to_owned()
            })?;
            validate_registered_work_symbol(fighter, &call.flag, IdentifierKind::WorkFlag)?;
        }
        ExcuteStmt::WorkTransitionTerm(call) => {
            let fighter = fighter
                .ok_or_else(|| "transition-term editing requires a concrete fighter".to_owned())?;
            super::native_work::validate_source_transition_term(fighter, call)?;
        }
        ExcuteStmt::WorkModuleIncInt(call) => {
            let fighter = fighter.ok_or_else(|| {
                "WorkModule increment editing requires a fighter registry binding".to_owned()
            })?;
            validate_registered_work_symbol(fighter, &call.slot, IdentifierKind::WorkInt)?;
        }
        ExcuteStmt::WorkModuleSet(call) => {
            let fighter = fighter.ok_or_else(|| {
                "WorkModule set editing requires a fighter registry binding".to_owned()
            })?;
            validate_work_module_set(fighter, call)?;
        }
        ExcuteStmt::SlowWhole(call) => {
            let fighter = fighter
                .ok_or_else(|| "SlowModule whole editing requires a concrete fighter".to_owned())?;
            if registry::fighter_kind_symbol(fighter).is_none() {
                return Err(format!(
                    "SlowModule whole editing requires a known concrete fighter, got {fighter:?}"
                ));
            }
            if let crate::data::SlowWholeCall::Set { multiplier, frames } = call {
                let measured = crate::data::SLOW_WHOLE_SET_PAIRS
                    .iter()
                    .any(|pair| pair.multiplier == *multiplier && pair.frames == *frames);
                if !measured {
                    return Err(format!(
                        "unmeasured SlowModule::set_whole pair ({multiplier}, {frames})"
                    ));
                }
            }
        }
        ExcuteStmt::SlowOpponent(call) => {
            validate_native_source_fighter(fighter, "SLOW_OPPONENT")?;
            if !call.is_valid() {
                return Err(format!(
                    "SLOW_OPPONENT inputs must be finite and within observed bounds: slow_mul {}..={}, slow_time {}..={}",
                    crate::data::SLOW_OPPONENT_SLOW_MUL_MIN,
                    crate::data::SLOW_OPPONENT_SLOW_MUL_MAX,
                    crate::data::SLOW_OPPONENT_SLOW_TIME_MIN,
                    crate::data::SLOW_OPPONENT_SLOW_TIME_MAX
                ));
            }
        }
        ExcuteStmt::OptionalHitFeedback(call) => {
            validate_native_source_fighter(fighter, "optional hit feedback")?;
            if !call.is_valid() {
                return Err(format!(
                    "optional hit feedback arg2 must be {}..={} and Hash40 must be a non-empty name of at most {} UTF-8 bytes or a raw u64",
                    crate::data::OptionalHitFeedbackCall::MIN_ARG2,
                    crate::data::OptionalHitFeedbackCall::MAX_ARG2,
                    crate::data::Hash40Literal::MAX_NAME_BYTES
                ));
            }
        }
        ExcuteStmt::ArticleLifecycle(call) => {
            let fighter = fighter.ok_or_else(|| {
                "article lifecycle editing requires a fighter registry binding".to_owned()
            })?;
            let symbol = match call {
                crate::data::ArticleLifecycleCall::Generate { symbol }
                | crate::data::ArticleLifecycleCall::GenerateEnabled { symbol }
                | crate::data::ArticleLifecycleCall::ChangeStatus { symbol, .. }
                | crate::data::ArticleLifecycleCall::RemoveAll { symbol }
                | crate::data::ArticleLifecycleCall::ChangeMotion { symbol, .. }
                | crate::data::ArticleLifecycleCall::SetVisibilityWhole { symbol, .. }
                | crate::data::ArticleLifecycleCall::Shoot { symbol, .. }
                | crate::data::ArticleLifecycleCall::ShootExist { symbol, .. } => symbol,
                crate::data::ArticleLifecycleCall::AddMotionPartial { preset } => {
                    if preset.fighter() != fighter {
                        return Err(format!(
                            "partial article motion preset belongs to {}, not {fighter}",
                            preset.fighter()
                        ));
                    }
                    preset.article_symbol()
                }
            };
            validate_article_symbol(fighter, symbol)?;
            if let crate::data::ArticleLifecycleCall::ChangeStatus {
                status, if_present, ..
            } = call
            {
                if !crate::data::article_status_catalog::is_measured_for(
                    fighter,
                    symbol,
                    status,
                    *if_present,
                ) {
                    return Err(format!("article state/API tuple is not measured for {fighter}: {symbol}, {status}, if_present={if_present}"));
                }
            }
            if matches!(
                call,
                crate::data::ArticleLifecycleCall::GenerateEnabled { .. }
            ) && !crate::data::is_measured_enabled_article(fighter, symbol)
            {
                return Err(format!(
                    "enabled article generation is not measured for {fighter}: {symbol}"
                ));
            }
            if let crate::data::ArticleLifecycleCall::ChangeMotion { motion, .. } = call {
                if !crate::data::measured_article_motions(symbol).contains(&motion.as_str()) {
                    return Err(format!(
                        "article motion {motion:?} is not measured for {symbol}"
                    ));
                }
            }
        }
        ExcuteStmt::GimmickChartEventValues {
            name, arguments, ..
        } => {
            if arguments.is_empty() {
                return Err("typed controller event requires one or more literal inputs".into());
            }
            crate::gimmicks::event_payload::validate_argument_shape("Move", name, arguments)
                .map_err(|errors| {
                    errors
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                })?;
            if let ExcuteStmt::GimmickChartEventValues { chart_id, name, .. } = command {
                validate_command(
                    fighter,
                    &ExcuteStmt::GimmickChartEvent {
                        chart_id: chart_id.clone(),
                        name: name.clone(),
                    },
                )?;
            }
        }
        ExcuteStmt::GimmickChartEvent { chart_id, name } => {
            if chart_id == "main"
                || chart_id.trim().is_empty()
                || chart_id.len() > crate::gimmicks::controller_protocol::MAX_CHART_ID_BYTES
                || chart_id.contains('\0')
            {
                return Err("controller event requires a nonblank, NUL-free additional-chart identity of at most 48 bytes".into());
            }
            if name.trim().is_empty()
                || name.len() > crate::gimmicks::live_model::MAX_NAME_BYTES
                || name.contains('\0')
            {
                return Err(
                    "controller event requires a bounded, nonblank, NUL-free event name".into(),
                );
            }
        }
        ExcuteStmt::GimmickEvent { name } => {
            if name.trim().is_empty()
                || name.len() > crate::gimmicks::live_model::MAX_NAME_BYTES
                || name.contains('\0')
            {
                return Err(format!(
                    "gimmick event name must be non-empty, NUL-free, and at most {} bytes",
                    crate::gimmicks::live_model::MAX_NAME_BYTES
                ));
            }
        }
        ExcuteStmt::VisibilitySetInt64 { group, status } => {
            validate_quoted_name(group, "visibility group")?;
            validate_quoted_name(status, "visibility status")?;
            if crate::data::body_visibility_catalog::is_legacy_pair(group, status) {
                return Ok(());
            }
            let fighter = fighter.ok_or_else(|| {
                "visibility editing requires a measured source-owner binding".to_owned()
            })?;
            if !crate::data::body_visibility_catalog::is_measured_for(fighter, group, status) {
                return Err(format!(
                    "visibility pair ({group:?}, {status:?}) is not measured for {fighter:?}"
                ));
            }
        }
        ExcuteStmt::VisibilitySetDefaultInt64 { group } => {
            validate_quoted_name(group, "model-default visibility group")?;
            let fighter = fighter.ok_or_else(|| {
                "model-default visibility editing requires a measured source-owner binding"
                    .to_owned()
            })?;
            if !crate::data::is_measured_default_int64_group(fighter, group) {
                return Err(format!(
                    "model-default visibility group {group:?} is not measured for {fighter:?}"
                ));
            }
        }
        ExcuteStmt::FixedJostleArea(call) => {
            let values: &[f32] = match call {
                crate::data::FixedJostleAreaCall::EnableFix { arg2, arg3 } => &[*arg2, *arg3],
                crate::data::FixedJostleAreaCall::EnableFixXy {
                    arg2,
                    arg3,
                    arg4,
                    arg5,
                } => &[*arg2, *arg3, *arg4, *arg5],
            };
            if values.iter().any(|value| {
                !value.is_finite() || value.abs() > crate::data::MAX_FIXED_JOSTLE_AREA_ARGUMENT
            }) {
                return Err(
                    "fixed jostle-area arguments must be finite and within the supported bound"
                        .into(),
                );
            }
        }
        ExcuteStmt::AttackModuleSetAll { kind, arg2, arg3 } => {
            use crate::data::AttackModuleSetAllKind;
            let observed_shape = match kind {
                AttackModuleSetAllKind::NoDeadAll => *arg2,
                AttackModuleSetAllKind::CatchOnlyAll => *arg2 && !*arg3,
            };
            if !observed_shape {
                return Err(
                    "AttackModule all-setter arguments are outside the measured source shapes"
                        .into(),
                );
            }
        }
        ExcuteStmt::AttackModuleSetIndexed { id, arg3, arg4, .. }
            if !(0..=3).contains(id) || !*arg3 || *arg4 =>
        {
            return Err(
                "indexed AttackModule setters only accept measured IDs 0–3 with true, false tails"
                    .into(),
            );
        }
        // The receiver and bool literal are fixed by the parser and serialization type.
        ExcuteStmt::AttackModuleSetIndexed { .. } | ExcuteStmt::GrabRebound { .. } => {}
        ExcuteStmt::AttackModuleSetHeightAll { arg3, .. } if *arg3 => {
            return Err("AttackModule height setter only accepts the measured false tail".into());
        }
        ExcuteStmt::AttackModuleSetHeightAll { .. } => {}
        ExcuteStmt::AttackModuleSetFinalFinishCutIn { id, arg5, .. }
            if *id < 0 || !arg5.is_finite() =>
        {
            return Err(
                "final-finish calls require a nonnegative attack ID and finite timing".into(),
            );
        }
        ExcuteStmt::AttackModuleSetFinalFinishCutIn { .. } => {}
        ExcuteStmt::AttackModuleInitAttackPos { id } if *id != 0 => {
            return Err("AttackModule::init_attack_pos only accepts the measured ID 0".into());
        }
        ExcuteStmt::AttackModuleInitAttackPos { .. }
        | ExcuteStmt::AttackModuleSetAttackReferenceJointId { .. } => {}
        ExcuteStmt::AttackModuleSetNoDamageFlySmokeAll { arg2, arg3 } if !*arg2 || *arg3 => {
            return Err(
                "AttackModule::set_no_damage_fly_smoke_all only accepts the measured true, false pair"
                    .into(),
            );
        }
        ExcuteStmt::AttackModuleSetNoDamageFlySmokeAll { .. } => {}
        ExcuteStmt::AttackModuleSetDamageShakeScale(scale)
            if !crate::data::AttackModuleDamageShakeScale::ALL.contains(scale) =>
        {
            return Err("damage shake scale is not one of the measured float literals".into());
        }
        ExcuteStmt::AttackModuleSetDamageShakeScale(_) => {}
        ExcuteStmt::AttackModuleSetNoUniqEffectAll(pair)
            if !crate::data::AttackModuleNoUniqEffectAllPair::ALL.contains(pair) =>
        {
            return Err("no-unique-effect arguments are not a measured boolean pair".into());
        }
        ExcuteStmt::AttackModuleSetNoUniqEffectAll(_) => {}
        ExcuteStmt::AttackModuleSetTargetCategory(id) => {
            let fighter = fighter.ok_or_else(|| {
                "AttackModule::set_target_category editing requires a concrete fighter".to_owned()
            })?;
            if registry::fighter_kind_symbol(fighter).is_none() {
                return Err(format!(
                    "AttackModule::set_target_category editing requires a known concrete fighter, got {fighter:?}"
                ));
            }
            if !crate::data::AttackModuleTargetCategoryId::ALL.contains(id) {
                return Err("target-category attack ID is not one of the measured IDs 0–2".into());
            }
        }
        ExcuteStmt::HitModuleSetStatusAllNormal => {
            validate_native_source_fighter(fighter, "HitModule::set_status_all")?;
        }
        ExcuteStmt::AttackModuleSetSize(preset) => {
            validate_native_source_fighter(fighter, "AttackModule::set_size")?;
            if !crate::data::AttackModuleSizePreset::ALL.contains(preset) {
                return Err("AttackModule::set_size tuple is not an exact measured preset".into());
            }
        }
        ExcuteStmt::AttackModuleSetVecTargetPos(preset) => {
            validate_native_source_fighter(fighter, "AttackModule::set_vec_target_pos")?;
            if !crate::data::AttackModuleVecTargetPosPreset::ALL.contains(preset) {
                return Err(
                    "AttackModule::set_vec_target_pos tuple is not an exact measured preset".into(),
                );
            }
        }
        ExcuteStmt::AttackModuleAddReactionFrame { id, frames }
        | ExcuteStmt::AttackModuleAddReactionFrameRevised { id, frames }
            if *id > 8 || !frames.is_finite() =>
        {
            return Err(
                "Native reaction-frame calls only accept IDs 0–8 and finite frame values".into(),
            );
        }
        ExcuteStmt::AttackModuleAddReactionFrame { .. }
        | ExcuteStmt::AttackModuleAddReactionFrameRevised { .. } => {}
        ExcuteStmt::Attack(call) => {
            if !matches!(call.func.as_str(), "ATTACK" | "ATTACK_IGNORE_THROW") {
                return Err("unsupported ATTACK macro name".into());
            }
            validate_quoted_name(&call.bone_name, "bone_name")?;
            validate_quoted_name(&call.collision_attr, "collision_attr")?;
            validate_quoted_name(&call.sound_attr, "sound_attr")?;
        }
        ExcuteStmt::Catch(call) => {
            validate_quoted_name(&call.bone_name, "bone_name")?;
        }
        ExcuteStmt::AttackAbs(call) => {
            validate_quoted_name(&call.collision_attr, "collision_attr")?;
            validate_quoted_name(&call.sound_attr, "sound_attr")?;
        }
        // This variant owns the complete fixed target-read/application source bundle; it has
        // no caller-controlled tokens and is only valid as a Game execute command.
        ExcuteStmt::AttackAbsApplyThrow => {}
        ExcuteStmt::Search(call) => validate_quoted_name(&call.bone_name, "bone_name")?,
        ExcuteStmt::HitStatus { target, status } => {
            if let crate::data::HurtTarget::Bone(name) = target {
                validate_quoted_name(name, "bone target")?;
            }
            validate_hit_status(status)?;
        }
        ExcuteStmt::Slope(crate::data::SlopeCall::Interpolate { frames, .. })
            if !(0..=crate::data::MAX_SLOPE_INTERPOLATION_FRAMES).contains(frames) =>
        {
            return Err("slope interpolation frame count is outside the supported range".into());
        }
        // SlopeCall and SlopeStatus are closed typed enums, and the optional tail
        // is already a bool. Validate their native frame bound above, then avoid
        // generic source-token validation: an omitted optional bool serializes to
        // JSON null, which is valid only for this typed slope representation.
        ExcuteStmt::Slope(_) => return Ok(()),
        ExcuteStmt::Wind(wind) => {
            if !wind.is_valid()
                || !wind.has_macro_wrapper()
                || wind.args.iter().any(|value| !value.is_finite())
            {
                return Err("unsupported or malformed wind command shape".into());
            }
        }
        ExcuteStmt::Sound(call) => {
            const SOUND_MACROS: &[&str] = &[
                "PLAY_SE",
                "PLAY_SE_NO_3D",
                "PLAY_SE_REMAIN",
                "STOP_SE",
                "PLAY_STEP",
                "PLAY_STEP_FLIPPABLE",
                "PLAY_SEQUENCE",
                "PLAY_STATUS",
                "PLAY_LANDING_SE",
                "PLAY_DOWN_SE",
                "PLAY_FLY_VOICE",
                "SET_PLAY_INHIVIT",
            ];
            if !SOUND_MACROS.contains(&call.func.as_str()) {
                return Err("unsupported sound macro name".into());
            }
            let (sound_count, has_tail) = match call.func.as_str() {
                "PLAY_STEP_FLIPPABLE" | "PLAY_FLY_VOICE" => (2, false),
                "SET_PLAY_INHIVIT" => (1, true),
                _ => (1, false),
            };
            if call.sounds.len() != sound_count || call.tail.is_some() != has_tail {
                return Err("malformed sound command arity".into());
            }
            for sound in &call.sounds {
                validate_quoted_name(sound, "sound name")?;
            }
        }
        ExcuteStmt::Expression(crate::data::ExpressionCall::ControlModuleSetRumble {
            receiver,
            kind,
            duration,
            looped,
            target,
        }) => {
            if !matches!(receiver.as_str(), "agent.module_accessor" | "boma") {
                return Err("rumble receiver must be agent.module_accessor or boma".into());
            }
            let kind_expr = syn::parse_str::<Expr>(kind.trim()).map_err(|error| {
                format!("rumble kind is not a supported Hash40 expression: {error}")
            })?;
            let measured_kind_shape = match &kind_expr {
                Expr::Call(call) => {
                    safe_call(call)
                        && matches!(call.func.as_ref(), Expr::Path(path)
                            if path.path.segments.last().is_some_and(|segment| segment.ident == "new"))
                }
                _ => false,
            };
            if !measured_kind_shape {
                return Err(
                    "rumble kind must use the measured Hash40::new string-literal form".into(),
                );
            }
            if !duration
                .trim()
                .parse::<i32>()
                .is_ok_and(|duration| duration >= 0)
            {
                return Err("rumble duration must be a nonnegative native i32 argument".into());
            }
            if !matches!(looped.trim(), "true" | "false") {
                return Err("rumble looped argument must be true or false".into());
            }
            if target.trim() != MEASURED_DIRECT_RUMBLE_TARGET {
                return Err(format!(
                    "rumble target is not the measured source token {MEASURED_DIRECT_RUMBLE_TARGET:?}"
                ));
            }
            // The sole measured target includes a native `as u32` cast. Permit that one
            // token without broadening the general source-expression validator.
            return Ok(());
        }
        ExcuteStmt::AttackFp(call) if call.args.len() != crate::data::ATTACK_FP_ARGC => {
            return Err(format!(
                "ATTACK_FP requires exactly {} arguments",
                crate::data::ATTACK_FP_ARGC
            ));
        }
        _ => {}
    }
    let value = serde_json::to_value(command).map_err(|err| err.to_string())?;
    validate_json(&value, "command")
}

fn validate_native_source_fighter(fighter: Option<&str>, family: &str) -> Result<(), String> {
    let fighter = fighter
        .ok_or_else(|| format!("{family} source editing requires a concrete registered fighter"))?;
    if registry::fighter_kind_symbol(fighter).is_none() {
        return Err(format!(
            "{family} source editing requires a known concrete fighter, got {fighter:?}"
        ));
    }
    Ok(())
}

fn validate_registered_work_symbol(
    fighter: &str,
    token: &str,
    kind: IdentifierKind,
) -> Result<(), String> {
    if kind == IdentifierKind::WorkFlag {
        return super::native_work::validate_flag(fighter, token).map(|_| ());
    }
    let symbol = token.trim().strip_prefix('*').ok_or_else(|| {
        "work identifier must use a dereferenced lua constant (*SYMBOL)".to_owned()
    })?;
    if symbol.is_empty()
        || !symbol
            .bytes()
            .enumerate()
            .all(|(index, byte)| byte.is_ascii_alphanumeric() || byte == b'_' && index > 0)
        || !symbol.as_bytes()[0].is_ascii_alphabetic() && symbol.as_bytes()[0] != b'_'
    {
        return Err("work identifier must be a named lua constant".into());
    }
    registry::resolve(fighter, symbol, kind)
        .map(|_| ())
        .map_err(|err| format!("work identifier {symbol:?} rejected: {err}"))
}

fn validate_article_symbol(fighter: &str, symbol: &str) -> Result<(), String> {
    if symbol.len() > crate::data::MAX_ARTICLE_SYMBOL_BYTES
        || !symbol.starts_with("FIGHTER_")
        || !symbol.contains("_GENERATE_ARTICLE_")
        || !symbol
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err("article must be a bare FIGHTER_*_GENERATE_ARTICLE_* symbol".into());
    }
    registry::resolve(fighter, symbol, IdentifierKind::Article)
        .map(|_| ())
        .map_err(|err| format!("article symbol {symbol:?} rejected: {err}"))
}

fn validate_work_module_set(
    fighter: &str,
    call: &crate::data::WorkModuleSetCall,
) -> Result<(), String> {
    if super::native_work::is_source_only_work_module_set(call) {
        return super::native_work::validate_source_only_work_module_set(fighter, call);
    }
    if !matches!(call.receiver.as_str(), "agent.module_accessor" | "boma") {
        return Err("WorkModule receiver must be agent.module_accessor or boma".into());
    }
    let (slot_kind, valid_value) = match call.kind {
        crate::data::WorkModuleSetKind::Int => (
            IdentifierKind::WorkInt,
            call.value.trim().parse::<i32>().is_ok(),
        ),
        crate::data::WorkModuleSetKind::Float => {
            (IdentifierKind::WorkFloat, valid_float_literal(&call.value))
        }
        crate::data::WorkModuleSetKind::Int64 => {
            return Err("WorkModule::set_int64 has no reviewed work-slot registry".into());
        }
    };
    if !valid_value {
        return Err("WorkModule set value must be a finite number of the setter's type".into());
    }
    validate_registered_work_symbol(fighter, &call.slot, slot_kind)
}

fn validate_hit_status(status: &str) -> Result<(), String> {
    let token = status.trim();
    if token.parse::<u8>().is_ok_and(|value| value <= 3) {
        return Ok(());
    }
    // The ACMD parser stores named statuses without their source `*` because the
    // emitter restores it. Authored nodes may still carry the dereference, so
    // accept either spelling but require an exact member of the closed table.
    let symbol = token.strip_prefix('*').unwrap_or(token);
    if crate::param_labels::const_value(crate::param_labels::HIT_STATUS, symbol).is_some() {
        Ok(())
    } else {
        Err(
            "hurtbox status must be an exact HIT_STATUS_NORMAL, INVINCIBLE, XLU, or OFF constant, optionally dereferenced, or its numeric value 0..=3"
                .into(),
        )
    }
}

fn valid_float_literal(value: &str) -> bool {
    let Ok(expression) = syn::parse_str::<Expr>(value.trim()) else {
        return false;
    };
    let (literal, negative) = match expression {
        Expr::Lit(syn::ExprLit {
            lit: Lit::Float(literal),
            ..
        }) => (literal, false),
        Expr::Unary(syn::ExprUnary {
            op: UnOp::Neg(_),
            expr,
            ..
        }) => match *expr {
            Expr::Lit(syn::ExprLit {
                lit: Lit::Float(literal),
                ..
            }) => (literal, true),
            _ => return false,
        },
        _ => return false,
    };
    if !matches!(literal.suffix(), "" | "f32") {
        return false;
    }
    let Ok(value) = literal.base10_parse::<f32>() else {
        return false;
    };
    let value = if negative { -value } else { value };
    value.is_finite()
}

fn command_matches_track(track: Track, command: &ExcuteStmt) -> bool {
    if matches!(command, ExcuteStmt::Effect(_)) {
        return track == Track::Effect;
    }
    if matches!(command, ExcuteStmt::NativeCall(_) | ExcuteStmt::WorkFlag(_)) {
        return true;
    }
    if let ExcuteStmt::WorkModuleSet(call) = command {
        return track == Track::Game || super::native_work::is_numeric_source_work_module_set(call);
    }
    if let ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::ChangeMotion {
        symbol,
        motion,
    }) = command
    {
        return match track {
            Track::Game => crate::data::measured_article_motions(symbol).contains(&motion.as_str()),
            Track::Expression => {
                crate::data::measured_expression_article_motions(symbol).contains(&motion.as_str())
            }
            Track::Sound | Track::Effect => false,
        };
    }
    if let ExcuteStmt::AttackModuleSetAttackReferenceJointId { kind } = command {
        return match track {
            Track::Game => crate::data::AttackReferenceJointKind::GAME_CHOICES.contains(kind),
            Track::Expression => {
                crate::data::AttackReferenceJointKind::EXPRESSION_CHOICES.contains(kind)
            }
            Track::Sound | Track::Effect => false,
        };
    }
    if matches!(command, ExcuteStmt::CaptureCutCommon) {
        return false;
    }
    if matches!(command, ExcuteStmt::StartInfoFlashEye) {
        return matches!(track, Track::Game | Track::Expression);
    }
    // These source-only native families must never be accepted from Sound, Expression, or Effect.
    if matches!(
        command,
        ExcuteStmt::OptionalHitFeedback(_)
            | ExcuteStmt::CheckFinishCamera(_)
            | ExcuteStmt::CheckFinalStartCamera(_)
            | ExcuteStmt::FinalFearFace(_)
            | ExcuteStmt::ShootItemBullet
            | ExcuteStmt::CameraZoomOut
            | ExcuteStmt::StartCutIn
    ) {
        return track == Track::Game;
    }
    match track {
        Track::Game => true,
        Track::Sound => matches!(command, ExcuteStmt::Sound(_)),
        Track::Expression => matches!(
            command,
            ExcuteStmt::Expression(_)
                | ExcuteStmt::HitStatus { .. }
                | ExcuteStmt::HitResetAll
                | ExcuteStmt::Slope(_)
                | ExcuteStmt::ItemVisibility { .. }
                | ExcuteStmt::VisibilitySetInt64 { .. }
                | ExcuteStmt::VisibilitySetDefaultInt64 { .. }
                | ExcuteStmt::VisibilitySetWhole { .. }
                | ExcuteStmt::Wind(_)
                | ExcuteStmt::EraseWind(_)
                | ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::Generate { .. },)
                | ExcuteStmt::ArticleLifecycle(
                    crate::data::ArticleLifecycleCall::GenerateEnabled { .. }
                )
                | ExcuteStmt::ArticleLifecycle(
                    crate::data::ArticleLifecycleCall::ChangeStatus { .. }
                )
                | ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::RemoveAll { .. })
                | ExcuteStmt::ArticleLifecycle(
                    crate::data::ArticleLifecycleCall::SetVisibilityWhole { .. },
                )
                | ExcuteStmt::ArticleLifecycle(
                    crate::data::ArticleLifecycleCall::ShootExist { .. },
                )
        ),
        Track::Effect => false,
    }
}

/// Keep bare placement limited to command families the source reader has measured outside an
/// `is_excute` block. In particular, authoring a bare collision command remains unsupported.
fn bare_command_matches_track(track: Track, command: &ExcuteStmt) -> bool {
    if matches!(command, ExcuteStmt::NativeCall(_)) {
        return true;
    }
    matches!(
        (track, command),
        (Track::Game, ExcuteStmt::DamageNoReaction(_))
            | (Track::Game, ExcuteStmt::CaptureCutCommon)
            | (Track::Game, ExcuteStmt::FtStartAdjustMotionFrame(_))
            | (Track::Sound, ExcuteStmt::Sound(_))
            | (Track::Expression, ExcuteStmt::Expression(_))
    )
}

pub(crate) fn valid_native_execute_frame(frame: f32) -> bool {
    frame.is_finite() && (0.0..=1_000_000.0).contains(&frame)
}

fn valid_motion_rate(rate: f32) -> bool {
    rate.is_finite() && rate > 0.0
}

fn validate_quoted_name(name: &str, field: &str) -> Result<(), String> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        Err(format!(
            "{field} must contain only ASCII letters, digits, and underscores"
        ))
    } else {
        Ok(())
    }
}

fn validate_json(value: &serde_json::Value, path: &str) -> Result<(), String> {
    match value {
        serde_json::Value::Null
            if matches!(
                path.rsplit('.').next(),
                Some("capsule_end" | "tail" | "sync")
            ) =>
        {
            // These are the only nullable fields on typed ExcuteStmt payloads. Float NaN/Inf
            // also serializes to JSON null, but appears at a numeric field and still fails.
            Ok(())
        }
        serde_json::Value::Null => {
            Err(format!("{path} contains a non-finite or unsupported value"))
        }
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => Ok(()),
        serde_json::Value::String(text)
            if path == "command.WorkModuleSet.value"
                && super::native_work::source_work_module_set_descriptors()
                    .iter()
                    .any(|descriptor| {
                        descriptor.kind.is_int64() && descriptor.values.contains(&text.as_str())
                    }) =>
        {
            // Owner/type/receiver validation ran before this token walk. Admit
            // only exact reviewed setter RHS strings, not arbitrary casts or
            // a broader hash40 call grammar in other authored fields.
            Ok(())
        }
        serde_json::Value::String(text) => validate_source_text(text)
            .map_err(|reason| format!("{path} source token `{text}` rejected: {reason}")),
        serde_json::Value::Array(values) => values
            .iter()
            .enumerate()
            .try_for_each(|(index, value)| validate_json(value, &format!("{path}[{index}]"))),
        serde_json::Value::Object(fields) => fields
            .iter()
            .try_for_each(|(key, value)| validate_json(value, &format!("{path}.{key}"))),
    }
}

fn mixed_matches_baseline(commands: &[MoveCommand], original: &[ExcuteStmt]) -> bool {
    commands.len() == original.len()
        && commands
            .iter()
            .zip(original)
            .all(|(current, source)| match current {
                MoveCommand::Editable(command) => {
                    !matches!(command, ExcuteStmt::Raw(_))
                        && std::mem::discriminant(command) == std::mem::discriminant(source)
                }
                MoveCommand::Preserved(command) => {
                    matches!(
                        (json_value(command), json_value(source)),
                        (Ok(current), Ok(original)) if current == original
                    )
                }
            })
}

fn effect_command_family_matches(
    original: &crate::data::EffectMacro,
    current: &crate::data::EffectMacro,
) -> bool {
    use crate::data::EffectMacro;

    match (original, current) {
        (
            EffectMacro::Effect {
                spawn_func: original_spawn,
                ..
            },
            EffectMacro::Effect {
                spawn_func: current_spawn,
                ..
            },
        ) => original_spawn == current_spawn,
        (
            EffectMacro::EffectFollowWork {
                work_symbol: original_slot,
                bone_name: original_bone,
                unk8: original_unk8,
                ..
            },
            EffectMacro::EffectFollowWork {
                work_symbol: current_slot,
                bone_name: current_bone,
                unk8: current_unk8,
                ..
            },
        ) => {
            original_slot == current_slot
                && original_bone == current_bone
                && original_unk8 == current_unk8
        }
        (EffectMacro::Control(original), EffectMacro::Control(current)) => {
            std::mem::discriminant(original) == std::mem::discriminant(current)
        }
        (
            EffectMacro::Color {
                command: original_command,
                ..
            },
            EffectMacro::Color {
                command: current_command,
                ..
            },
        ) => original_command == current_command,
        _ => std::mem::discriminant(original) == std::mem::discriminant(current),
    }
}

fn decrement_remaining(remaining: &mut HashMap<String, usize>, key: &str) -> bool {
    let Some(count) = remaining.get_mut(key) else {
        return false;
    };
    if *count == 0 {
        return false;
    }
    *count -= 1;
    if *count == 0 {
        remaining.remove(key);
    }
    true
}

fn validate_source_text(text: &str) -> Result<(), String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("empty source token".into());
    }
    // This is the one receiver path carried as data rather than as an ACMD expression.
    if trimmed == "agent.module_accessor" {
        return Ok(());
    }
    let expr = syn::parse_str::<Expr>(trimmed).map_err(|err| err.to_string())?;
    if safe_expr(&expr) {
        Ok(())
    } else {
        Err("expression is outside the supported literal, constant, and Hash40 forms".into())
    }
}

fn safe_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Int(_) | Lit::Float(_) | Lit::Bool(_) | Lit::Str(_),
            ..
        }) => true,
        Expr::Path(ExprPath {
            qself: None, path, ..
        }) => {
            let segments = path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>();
            !segments.is_empty()
                && segments.len() <= 6
                && segments.iter().all(|segment| {
                    segment
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                        && segment != "std"
                        && segment != "core"
                        && segment != "process"
                })
                && (segments.len() == 1
                    || segments[0].starts_with(|ch: char| ch.is_ascii_uppercase())
                    || segments[0] == "smash")
                && path
                    .segments
                    .iter()
                    .all(|segment| matches!(segment.arguments, syn::PathArguments::None))
        }
        Expr::Unary(unary) => {
            matches!(unary.op, UnOp::Neg(_) | UnOp::Not(_) | UnOp::Deref(_))
                && safe_expr(&unary.expr)
        }
        Expr::Paren(paren) => safe_expr(&paren.expr),
        Expr::Group(group) => safe_expr(&group.expr),
        Expr::Binary(binary) => {
            matches!(
                binary.op,
                syn::BinOp::Add(_)
                    | syn::BinOp::Sub(_)
                    | syn::BinOp::Mul(_)
                    | syn::BinOp::Div(_)
                    | syn::BinOp::Rem(_)
                    | syn::BinOp::BitAnd(_)
                    | syn::BinOp::BitOr(_)
                    | syn::BinOp::BitXor(_)
                    | syn::BinOp::Shl(_)
                    | syn::BinOp::Shr(_)
            ) && safe_expr(&binary.left)
                && safe_expr(&binary.right)
        }
        Expr::Call(call) => safe_call(call),
        _ => false,
    }
}

fn safe_call(call: &ExprCall) -> bool {
    let Expr::Path(path) = call.func.as_ref() else {
        return false;
    };
    let function = path
        .path
        .segments
        .get(1)
        .map(|segment| segment.ident.to_string());
    let args_match = match function.as_deref() {
        Some("new") => {
            call.args.len() == 1
                && matches!(
                    call.args.first(),
                    Some(Expr::Lit(ExprLit {
                        lit: Lit::Str(_),
                        ..
                    }))
                )
        }
        Some("new_raw") => {
            call.args.len() == 1
                && matches!(
                    call.args.first(),
                    Some(Expr::Lit(ExprLit {
                        lit: Lit::Int(_),
                        ..
                    }))
                )
        }
        _ => false,
    };
    if path.qself.is_some()
        || path.path.segments.len() != 2
        || path.path.segments[0].ident != "Hash40"
        || !args_match
        || path
            .path
            .segments
            .iter()
            .any(|segment| !matches!(segment.arguments, syn::PathArguments::None))
    {
        return false;
    }
    true
}

fn preview(track: Track, script: &AcmdScript, move_name: &str) -> String {
    match track {
        Track::Game => crate::acmd::preview_game_fn(script, move_name),
        Track::Sound => crate::acmd::preview_sound_fn(script, move_name),
        Track::Expression => crate::acmd::preview_expression_fn(script, move_name),
        Track::Effect => crate::acmd::preview_effect_structure_fn(script, move_name),
    }
}

fn json_value<T: serde::Serialize + ?Sized>(value: &T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|err| err.to_string())
}

fn effect_execute_witness_key(commands: &[crate::data::EffectMacro]) -> Result<String, String> {
    Ok(format!(
        "{MIXED_EFFECT_WITNESS_PREFIX}{}",
        json_value(commands)?
    ))
}

fn error(fighter: &str, subject: &str, reason: impl Into<String>) -> NodeError {
    NodeError {
        subject: format!("{fighter}.{subject}"),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{AcmdScript, AcmdStmt, AttackCall, SoundCall, WorkFlagAction, WorkFlagCall};
    use crate::move_nodes::test_support as fixture;
    use crate::move_nodes::test_support::assert_json_eq;

    fn graph(track: MoveTrack) -> MoveGraph {
        MoveGraph {
            tracks: vec![track],
            ..MoveGraph::default()
        }
    }

    fn roundtrip(track: Track, statements: Vec<AcmdStmt>) {
        let original = AcmdScript { stmts: statements };
        let imported = import(track, &original);
        let output = compile("mario", "attack11", &graph(imported)).unwrap();
        assert_json_eq!(&original, &output[0].1);
    }

    #[test]
    fn zero_iteration_source_loops_stay_immutable_no_ops_on_every_track() {
        for track in [Track::Game, Track::Sound, Track::Expression, Track::Effect] {
            let category = match track {
                Track::Game => "game",
                Track::Sound => "sound",
                Track::Expression => "expression",
                Track::Effect => "effect",
            };
            let source = format!(
                "unsafe extern \"C\" fn {category}_zero(agent: &mut L2CAgentBase) {{\n    for _ in 0..0 {{\n        wait(agent.lua_state_agent, 99.0);\n    }}\n    frame(agent.lua_state_agent, 2.0);\n}}\n"
            );
            let imported = if track == Track::Effect {
                super::super::effects::import_for_fighter(
                    "mario",
                    &crate::acmd::parse_effect_script(&source),
                )
            } else {
                let script = match track {
                    Track::Game => crate::acmd::parse_acmd_script(&source),
                    Track::Sound => crate::acmd::parse_sound_script(&source),
                    Track::Expression => crate::acmd::parse_expression_script(&source),
                    Track::Effect => unreachable!(),
                };
                import_for_fighter("mario", track, &script)
            };
            assert!(
                matches!(imported.nodes[0].kind, NodeKind::Preserved { .. }),
                "{track:?}"
            );
            assert!(
                matches!(
                    imported.baseline.as_slice(),
                    [AcmdStmt::Loop { count: 0, .. }]
                ),
                "{track:?}"
            );
            let compiled = compile("mario", "zero", &graph(imported)).unwrap();
            assert!(
                matches!(
                    compiled[0].1.stmts.as_slice(),
                    [AcmdStmt::Loop { count: 0, .. }, AcmdStmt::Frame(2.0)]
                ),
                "{track:?}"
            );
        }
    }

    #[test]
    fn exact_repeat_counts_survive_source_graph_source_on_every_track() {
        fn acmd_counts(stmts: &[AcmdStmt], out: &mut Vec<usize>) {
            for stmt in stmts {
                match stmt {
                    AcmdStmt::Loop { count, body, .. } => {
                        out.push(*count);
                        acmd_counts(body, out);
                    }
                    AcmdStmt::RawBlock { body, .. } => acmd_counts(body, out),
                    _ => {}
                }
            }
        }
        fn effect_counts(stmts: &[crate::data::EffectStmt], out: &mut Vec<usize>) {
            for stmt in stmts {
                match stmt {
                    crate::data::EffectStmt::Loop { count, body, .. } => {
                        out.push(*count);
                        effect_counts(body, out);
                    }
                    crate::data::EffectStmt::Cond { body, .. } => effect_counts(body, out),
                    _ => {}
                }
            }
        }

        for (track, count) in [
            (Track::Game, 22),
            (Track::Sound, 24),
            (Track::Expression, 49),
            (Track::Effect, 100),
        ] {
            let prefix = match track {
                Track::Game => "game",
                Track::Sound => "sound",
                Track::Expression => "expression",
                Track::Effect => "effect",
            };
            let source = format!(
                "unsafe extern \"C\" fn {prefix}_repeat(agent: &mut L2CAgentBase) {{\n    for _ in 0..{count} {{\n        frame(agent.lua_state_agent, 1.0);\n    }}\n}}\n"
            );
            let imported = if track == Track::Effect {
                let parsed = crate::acmd::parse_effect_script(&source);
                let mut counts = Vec::new();
                effect_counts(&parsed.stmts, &mut counts);
                assert_eq!(counts, [count]);
                crate::move_nodes::effects::import_for_fighter("mario", &parsed)
            } else {
                let parsed = match track {
                    Track::Game => crate::acmd::parse_acmd_script(&source),
                    Track::Sound => crate::acmd::parse_sound_script(&source),
                    Track::Expression => crate::acmd::parse_expression_script(&source),
                    Track::Effect => unreachable!(),
                };
                let mut counts = Vec::new();
                acmd_counts(&parsed.stmts, &mut counts);
                assert_eq!(counts, [count], "parser {track:?}");
                import_for_fighter("mario", track, &parsed)
            };
            assert!(
                matches!(
                    imported.nodes.as_slice(),
                    [MoveNode {
                        kind: NodeKind::Repeat { count: actual, .. },
                        ..
                    }] if *actual == count as u32
                ),
                "graph import {track:?}"
            );
            let compiled = compile("mario", "repeat", &graph(imported)).unwrap();
            assert_eq!(compiled[0].0, track);
            let emitted = match track {
                Track::Game => crate::acmd::preview_game_fn(&compiled[0].1, "repeat"),
                Track::Sound => crate::acmd::preview_sound_fn(&compiled[0].1, "repeat"),
                Track::Expression => crate::acmd::preview_expression_fn(&compiled[0].1, "repeat"),
                Track::Effect => crate::acmd::preview_effect_structure_fn(&compiled[0].1, "repeat"),
            };
            if track == Track::Effect {
                let reparsed = crate::acmd::parse_effect_script(&emitted);
                let mut counts = Vec::new();
                effect_counts(&reparsed.stmts, &mut counts);
                assert_eq!(counts, [count], "source round-trip {track:?}");
            } else {
                let reparsed = match track {
                    Track::Game => crate::acmd::parse_acmd_script(&emitted),
                    Track::Sound => crate::acmd::parse_sound_script(&emitted),
                    Track::Expression => crate::acmd::parse_expression_script(&emitted),
                    Track::Effect => unreachable!(),
                };
                let mut counts = Vec::new();
                acmd_counts(&reparsed.stmts, &mut counts);
                assert_eq!(counts, [count], "source round-trip {track:?}");
            }
        }
    }

    #[test]
    fn nested_named_events_promote_only_inside_clean_game_branches_and_small_repeats() {
        use crate::move_nodes::model::{InflictionMask, InflictionQuery};

        fn typed_event_names(nodes: &[MoveNode], names: &mut Vec<String>) {
            for node in nodes {
                match &node.kind {
                    NodeKind::Execute { commands } => {
                        names.extend(commands.iter().filter_map(|command| match command {
                            ExcuteStmt::GimmickEvent { name } => Some(name.clone()),
                            _ => None,
                        }))
                    }
                    NodeKind::Branch {
                        then_nodes,
                        else_nodes,
                        ..
                    } => {
                        typed_event_names(then_nodes, names);
                        typed_event_names(else_nodes, names);
                    }
                    NodeKind::Repeat { body, .. } => typed_event_names(body, names),
                    _ => {}
                }
            }
        }

        let condition = MoveCondition::And {
            left: Box::new(MoveCondition::AttackInfliction {
                query: InflictionQuery::CurrentStatus,
                mask: InflictionMask::Hit,
            }),
            right: Box::new(MoveCondition::Or {
                left: Box::new(MoveCondition::WorkFlag {
                    symbol: "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO".into(),
                    expected: true,
                }),
                right: Box::new(MoveCondition::ButtonHeld {
                    button: InputButton::Attack,
                }),
            }),
        };
        let header = crate::move_nodes::compound_conditions::header("mario", &condition).unwrap();
        let source = format!(
            "unsafe extern \"C\" fn game_event_controls(agent: &mut L2CAgentBase) {{\n    {header}\n        if macros::is_excute(agent) {{\n            crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, \"hit\");\n        }}\n    }} else {{\n        if macros::is_excute(agent) {{\n            crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, \"else_hit\");\n        }}\n    }}\n    for _ in 0..2 {{\n        if macros::is_excute(agent) {{\n            crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, \"repeat_hit\");\n        }}\n    }}\n}}\n"
        );
        let parsed = crate::acmd::parse_acmd_script(&source);
        let imported = import_for_fighter("mario", Track::Game, &parsed);
        let mut names = Vec::new();
        typed_event_names(&imported.nodes, &mut names);
        assert_eq!(names, vec!["hit", "else_hit", "repeat_hit"]);
        assert!(matches!(
            imported.nodes.as_slice(),
            [
                MoveNode {
                    kind: NodeKind::Branch { else_nodes, .. },
                    ..
                },
                MoveNode {
                    kind: NodeKind::Repeat { count: 2, .. },
                    ..
                }
            ] if !else_nodes.is_empty()
        ));

        let compiled = compile("mario", "event_controls", &graph(imported.clone())).unwrap();
        assert_eq!(compiled[0].0, Track::Game);
        let emitted = crate::acmd::preview_game_fn(&compiled[0].1, "event_controls");
        let reparsed = crate::acmd::parse_acmd_script(&emitted);
        let reimported = import_for_fighter("mario", Track::Game, &reparsed);
        assert_json_eq!(imported, reimported, "typed send, condition, repeat count and paired else should survive graph/source round-trip");
    }

    #[test]
    fn mixed_nested_event_execute_normalizes_only_its_witness_and_preserves_neighbors() {
        let source = r#"unsafe extern "C" fn game_mixed_event(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO) {
        if macros::is_excute(agent) {
            crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "before");
            AttackModule::set_no_finish_camera(agent.module_accessor, 0, true, false);
            custom_unknown_neighbor(agent.module_accessor, 7);
            VisibilityModule::set_int64(agent.module_accessor, hash40("hammer") as i64, hash40("hammer_invisible") as i64);
        }
    }
}
"#;
        let parsed = crate::acmd::parse_acmd_script(source);
        let mut imported = import_for_fighter("mario", Track::Game, &parsed);
        let NodeKind::Branch { then_nodes, .. } = &mut imported.nodes[0].kind else {
            panic!("clean parent branch should import");
        };
        let NodeKind::MixedExecute { commands } = &mut then_nodes[0].kind else {
            panic!("the exact send and native-family neighbor form a mixed execute");
        };
        assert!(
            matches!(
                commands.as_slice(),
                [
                    MoveCommand::Editable(ExcuteStmt::GimmickEvent { name }),
                    MoveCommand::Preserved(ExcuteStmt::Raw(attack_raw)),
                    MoveCommand::Preserved(ExcuteStmt::Raw(unknown_raw)),
                    MoveCommand::Preserved(ExcuteStmt::Raw(visibility_raw)),
                ]
                    if name == "before"
                        && attack_raw
                            == "AttackModule::set_no_finish_camera(agent.module_accessor, 0, true, false);"
                        && unknown_raw == "custom_unknown_neighbor(agent.module_accessor, 7);"
                        && visibility_raw.starts_with("VisibilityModule::set_int64(")
            ),
            "named send promotion must not grant nested native AttackModule parsing"
        );
        let MoveCommand::Editable(ExcuteStmt::GimmickEvent { name }) = &mut commands[0] else {
            unreachable!();
        };
        *name = "after".into();
        let authored = graph(imported);
        let compiled = compile("mario", "mixed_event", &authored).unwrap();
        let emitted = crate::acmd::preview_game_fn(&compiled[0].1, "mixed_event");
        assert!(emitted.contains("dispatch_named_from_module(agent.module_accessor, \"after\");"));
        assert!(emitted.contains(
            "AttackModule::set_no_finish_camera(agent.module_accessor, 0, true, false);"
        ));
        assert!(emitted.contains("custom_unknown_neighbor(agent.module_accessor, 7);"));
        assert!(emitted.contains("VisibilityModule::set_int64("));
        let reparsed = crate::acmd::parse_acmd_script(&emitted);
        let reimported = import_for_fighter("mario", Track::Game, &reparsed);
        assert_json_eq!(
            &authored.tracks[0].nodes,
            &reimported.nodes,
            "only the typed event name is editable; the native neighbor remains immutable"
        );
        // A fresh import refreshes the editable event's witness to the new source name;
        // comparing the old and new baseline names would incorrectly forbid the edit.
        assert!(compile("mario", "mixed_event", &graph(reimported)).is_ok());
        for mutation in 0..3 {
            let mut invalid = authored.clone();
            let NodeKind::Branch { then_nodes, .. } = &mut invalid.tracks[0].nodes[0].kind else {
                unreachable!();
            };
            let NodeKind::MixedExecute { commands } = &mut then_nodes[0].kind else {
                unreachable!();
            };
            match mutation {
                0 => {
                    commands[1] =
                        MoveCommand::Preserved(ExcuteStmt::Raw("modified_native();".into()))
                }
                1 => {
                    commands.remove(2);
                }
                _ => commands.swap(1, 2),
            }
            assert!(
                compile("mario", "mixed_event", &invalid).is_err(),
                "preserved slot mutation {mutation}"
            );
        }
    }

    #[test]
    fn nested_event_scopes_do_not_type_attack_or_root_visibility_writes() {
        let source = crate::acmd::parse_acmd_script(
            r#"unsafe extern "C" fn game_event_native_neighbors(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO) {
        if macros::is_excute(agent) {
            crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "event");
            AttackModule::set_no_finish_camera(agent.module_accessor, 0, true, false);
            VisibilityModule::set_int64(agent.module_accessor, hash40("hammer") as i64, hash40("hammer_invisible") as i64);
        }
    }
}
"#,
        );
        fn count_typed_native_writes(statements: &[AcmdStmt]) -> usize {
            statements
                .iter()
                .map(|statement| match statement {
                    AcmdStmt::Excute(commands) => commands
                        .iter()
                        .filter(|command| {
                            matches!(
                                command,
                                ExcuteStmt::AttackModuleSetIndexed { .. }
                                    | ExcuteStmt::VisibilitySetInt64 { .. }
                            )
                        })
                        .count(),
                    AcmdStmt::RawBlock { body, .. } | AcmdStmt::Loop { body, .. } => {
                        count_typed_native_writes(body)
                    }
                    _ => 0,
                })
                .sum()
        }
        assert_eq!(count_typed_native_writes(&source.stmts), 0);

        let imported = import_for_fighter("mario", Track::Game, &source);
        let NodeKind::Branch { then_nodes, .. } = &imported.nodes[0].kind else {
            panic!("the proven Game condition should import");
        };
        let NodeKind::MixedExecute { commands } = &then_nodes[0].kind else {
            panic!("only the named event should become editable");
        };
        assert!(matches!(
            commands.as_slice(),
            [
                MoveCommand::Editable(ExcuteStmt::GimmickEvent { .. }),
                MoveCommand::Preserved(ExcuteStmt::Raw(attack_raw)),
                MoveCommand::Preserved(ExcuteStmt::Raw(visibility_raw)),
            ] if attack_raw.contains("AttackModule::set_no_finish_camera")
                && visibility_raw.contains("VisibilityModule::set_int64")
        ));
    }

    #[test]
    fn nested_event_source_stays_raw_for_unknown_attributed_and_non_game_contexts() {
        fn editable_event_count(nodes: &[MoveNode]) -> usize {
            nodes
                .iter()
                .map(|node| match &node.kind {
                    NodeKind::Execute { commands } => commands
                        .iter()
                        .filter(|command| matches!(command, ExcuteStmt::GimmickEvent { .. }))
                        .count(),
                    NodeKind::Branch {
                        then_nodes,
                        else_nodes,
                        ..
                    } => editable_event_count(then_nodes) + editable_event_count(else_nodes),
                    NodeKind::Repeat { body, .. } => editable_event_count(body),
                    _ => 0,
                })
                .sum()
        }

        fn assert_source_ast_roundtrip(
            source: &str,
            track: Track,
            original: &crate::data::AcmdScript,
            case: &str,
        ) {
            let imported = import_for_fighter("mario", track, original);
            let compiled = compile("mario", "event_negative", &graph(imported)).unwrap();
            let emitted = if track == Track::Expression {
                crate::acmd::preview_expression_fn(&compiled[0].1, "event_negative")
            } else {
                crate::acmd::preview_game_fn(&compiled[0].1, "event_negative")
            };
            let reparsed = if track == Track::Expression {
                crate::acmd::parse_expression_script(&emitted)
            } else {
                crate::acmd::parse_acmd_script(&emitted)
            };
            assert_json_eq!(
                &original.stmts,
                &reparsed.stmts,
                "{case}: parsed source AST should survive graph compile, emission, and reparse"
            );
            assert!(
                emitted.contains("dispatch_named_from_module"),
                "{case}: {source}"
            );
        }

        let unknown = crate::acmd::parse_acmd_script(
            r#"unsafe extern "C" fn game_unknown_event(agent: &mut L2CAgentBase) {
    if custom_condition(agent) {
        if macros::is_excute(agent) {
            crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "unknown");
        }
    }
}
"#,
        );
        let unknown_graph = import_for_fighter("mario", Track::Game, &unknown);
        assert_eq!(editable_event_count(&unknown_graph.nodes), 0);
        assert!(unknown_graph
            .nodes
            .first()
            .is_some_and(|node| matches!(&node.kind, NodeKind::Preserved { .. })));
        assert_source_ast_roundtrip(
            r#"unknown condition"#,
            Track::Game,
            &unknown,
            "unknown condition",
        );

        let attributed_parent = crate::acmd::parse_acmd_script(
            r#"unsafe extern "C" fn game_attributed_parent(agent: &mut L2CAgentBase) {
    #[cfg(any())]
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO) {
        if macros::is_excute(agent) {
            crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "parent_attr");
        }
    }
}
"#,
        );
        let attributed_graph = import_for_fighter("mario", Track::Game, &attributed_parent);
        assert_eq!(editable_event_count(&attributed_graph.nodes), 0);
        assert!(attributed_graph
            .nodes
            .first()
            .is_some_and(|node| matches!(&node.kind, NodeKind::Preserved { .. })));
        assert_source_ast_roundtrip(
            "attributed parent",
            Track::Game,
            &attributed_parent,
            "attributed parent",
        );

        let attributed_execute = crate::acmd::parse_acmd_script(
            r#"unsafe extern "C" fn game_attributed_execute(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO) {
        #[cfg(any())]
        if macros::is_excute(agent) {
            crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "execute_attr");
        }
    }
}
"#,
        );
        let attributed_execute_graph =
            import_for_fighter("mario", Track::Game, &attributed_execute);
        assert_eq!(editable_event_count(&attributed_execute_graph.nodes), 0);
        assert_source_ast_roundtrip(
            "attributed Execute",
            Track::Game,
            &attributed_execute,
            "attributed Execute",
        );

        let attributed_call = crate::acmd::parse_acmd_script(
            r#"unsafe extern "C" fn game_attributed_call(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO) {
        if macros::is_excute(agent) {
            #[cfg(any())]
            crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "call_attr");
        }
    }
}
"#,
        );
        let attributed_call_graph = import_for_fighter("mario", Track::Game, &attributed_call);
        assert_eq!(editable_event_count(&attributed_call_graph.nodes), 0);
        assert_source_ast_roundtrip(
            "attributed call",
            Track::Game,
            &attributed_call,
            "attributed call",
        );

        for source in [
            r#"unsafe extern "C" fn game_event_empty_else(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO) {
        if macros::is_excute(agent) {
            crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "empty_else");
        }
    } else {
    }
}
"#,
            r#"unsafe extern "C" fn game_event_else_if(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO) {
        if macros::is_excute(agent) {
            crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "else_if");
        }
    } else if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_AIR {
        if macros::is_excute(agent) {
            crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "else_if_child");
        }
    }
}
"#,
        ] {
            let parsed = crate::acmd::parse_acmd_script(source);
            let imported = import_for_fighter("mario", Track::Game, &parsed);
            assert_eq!(editable_event_count(&imported.nodes), 0);
            assert_source_ast_roundtrip(source, Track::Game, &parsed, "unsupported else topology");
        }

        let attributed_loop = crate::acmd::parse_acmd_script(
            r#"unsafe extern "C" fn game_attributed_loop(agent: &mut L2CAgentBase) {
    #[cfg(any())]
    for _ in 0..2 {
        if macros::is_excute(agent) {
            crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "loop_attr");
        }
    }
}
"#,
        );
        let attributed_loop_graph = import_for_fighter("mario", Track::Game, &attributed_loop);
        assert_eq!(editable_event_count(&attributed_loop_graph.nodes), 0);
        assert_source_ast_roundtrip(
            "attributed loop",
            Track::Game,
            &attributed_loop,
            "attributed loop",
        );

        let malformed = crate::acmd::parse_acmd_script(
            r#"unsafe extern "C" fn game_malformed_event(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO) {
        if macros::is_excute(agent) {
            crate::slight::gimmicks::dispatch_named_from_module(boma, "alias");
        }
    }
}
"#,
        );
        let malformed_graph = import_for_fighter("mario", Track::Game, &malformed);
        assert_eq!(editable_event_count(&malformed_graph.nodes), 0);
        assert_source_ast_roundtrip(
            "malformed receiver",
            Track::Game,
            &malformed,
            "malformed receiver",
        );

        let expression = crate::acmd::parse_expression_script(
            r#"unsafe extern "C" fn expression_event(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO) {
        if macros::is_excute(agent) {
            crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "not_game");
        }
    }
}
"#,
        );
        let expression_graph = import_for_fighter("mario", Track::Expression, &expression);
        assert_eq!(editable_event_count(&expression_graph.nodes), 0);
        assert_source_ast_roundtrip(
            "non-Game track",
            Track::Expression,
            &expression,
            "non-Game track",
        );

        for source in [
            r#"unsafe extern "C" fn game_multiline_attr_parent(agent: &mut L2CAgentBase) {
    #[cfg(
        any()
    )] // trivia must not detach the attribute
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO) {
        if macros::is_excute(agent) {
            crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "parent");
        }
    }
}
"#,
            r#"unsafe extern "C" fn game_multiline_attr_call(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO) {
        if macros::is_excute(agent) {
            #[cfg(
                any()
            )] // call-level trivia
            crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "call");
        }
    }
}
"#,
            r#"unsafe extern "C" fn game_multiline_attr_execute(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO) {
        #[cfg(
            any()
        )] // execute-level trivia
        if macros::is_excute(agent) {
            crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, "execute");
        }
    }
}
"#,
        ] {
            let original = crate::acmd::parse_acmd_script(source);
            let imported = import_for_fighter("mario", Track::Game, &original);
            assert_eq!(editable_event_count(&imported.nodes), 0);
            let compiled = compile("mario", "attributed_event", &graph(imported)).unwrap();
            let emitted = crate::acmd::preview_game_fn(&compiled[0].1, "attributed_event");
            let reparsed = crate::acmd::parse_acmd_script(&emitted);
            assert_json_eq!(
                &original,
                reparsed,
                "attributed event contexts should survive source -> graph -> source unchanged"
            );
            assert_source_ast_roundtrip(
                "multiline attributed source",
                Track::Game,
                &original,
                "multiline attributed source",
            );
        }
    }

    #[test]
    fn attack_infliction_leaves_are_game_only_closed_source_conditions() {
        use crate::data::{WorkFlagAction, WorkFlagCall};
        use crate::move_nodes::model::{InflictionMask, InflictionQuery};

        for (query, mask, getter, symbol) in [
            (
                InflictionQuery::CurrentHitlag,
                InflictionMask::Hit,
                "is_infliction",
                "HIT",
            ),
            (
                InflictionQuery::CurrentHitlag,
                InflictionMask::Shield,
                "is_infliction",
                "SHIELD",
            ),
            (
                InflictionQuery::CurrentStatus,
                InflictionMask::Hit,
                "is_infliction_status",
                "HIT",
            ),
            (
                InflictionQuery::CurrentStatus,
                InflictionMask::Shield,
                "is_infliction_status",
                "SHIELD",
            ),
        ] {
            let header = format!(
                "if AttackModule::{getter}(agent.module_accessor, *COLLISION_KIND_MASK_{symbol}) {{"
            );
            let expected = MoveCondition::AttackInfliction { query, mask };
            assert_json_eq!(import_condition(Some("mario"), &header), Some(&expected));
            assert_eq!(
                crate::move_nodes::compound_conditions::header("mario", &expected).unwrap(),
                header
            );

            let source = AcmdScript {
                stmts: vec![
                    AcmdStmt::RawBlock {
                        header: header.clone(),
                        body: vec![
                            AcmdStmt::Frame(1.0),
                            AcmdStmt::Wait(0.5),
                            AcmdStmt::Excute(vec![ExcuteStmt::WorkFlag(WorkFlagCall {
                                action: WorkFlagAction::On,
                                flag: "*FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO".into(),
                            })]),
                        ],
                    },
                    AcmdStmt::RawBlock {
                        header: "else {".into(),
                        body: vec![
                            AcmdStmt::Frame(2.0),
                            AcmdStmt::Wait(0.25),
                            AcmdStmt::Excute(vec![ExcuteStmt::WorkFlag(WorkFlagCall {
                                action: WorkFlagAction::Off,
                                flag: "*FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO".into(),
                            })]),
                        ],
                    },
                ],
            };
            let imported = import_for_fighter("mario", Track::Game, &source);
            assert!(matches!(
                imported.nodes.first().map(|node| &node.kind),
                Some(NodeKind::Branch { condition, .. }) if serde_json::to_value(condition).unwrap() == serde_json::to_value(expected).unwrap()
            ));
            let graph = graph(imported);
            assert_eq!(
                super::super::live_support::source_only_tracks(&graph).collect::<Vec<_>>(),
                vec![Track::Game]
            );

            let compiled = compile("mario", "attack11", &graph).unwrap();
            assert_eq!(compiled.len(), 1);
            assert_eq!(compiled[0].0, Track::Game);
            assert_json_eq!(&compiled[0].1, &source);
            let emitted = crate::acmd::preview_game_fn(&compiled[0].1, "attack11");
            let parsed = crate::acmd::parse_acmd_script(&emitted);
            assert_json_eq!(parsed, source);
        }

        let compound = MoveCondition::Not(Box::new(MoveCondition::Group(Box::new(
            MoveCondition::And {
                left: Box::new(MoveCondition::AttackInfliction {
                    query: InflictionQuery::CurrentStatus,
                    mask: InflictionMask::Hit,
                }),
                right: Box::new(MoveCondition::AttackInfliction {
                    query: InflictionQuery::CurrentHitlag,
                    mask: InflictionMask::Shield,
                }),
            },
        ))));
        let compound_header =
            crate::move_nodes::compound_conditions::header("mario", &compound).unwrap();
        assert_eq!(
            compound_header,
            "if !(AttackModule::is_infliction_status(agent.module_accessor, *COLLISION_KIND_MASK_HIT) && AttackModule::is_infliction(agent.module_accessor, *COLLISION_KIND_MASK_SHIELD)) {"
        );
        assert_json_eq!(
            crate::move_nodes::compound_conditions::import("mario", &compound_header),
            Some(compound)
        );

        let sound_header =
            "if AttackModule::is_infliction(agent.module_accessor, *COLLISION_KIND_MASK_HIT) {";
        let sound_source = AcmdScript {
            stmts: vec![AcmdStmt::RawBlock {
                header: sound_header.into(),
                body: vec![AcmdStmt::Wait(1.0)],
            }],
        };
        let sound_imported = import_for_fighter("mario", Track::Sound, &sound_source);
        assert!(matches!(
            sound_imported.nodes.first().map(|node| &node.kind),
            Some(NodeKind::Preserved { .. })
        ));
        assert_json_eq!(&sound_imported.baseline, &sound_source.stmts);

        let authored_sound = fixture::track(
            Track::Sound,
            vec![fixture::node(
                1,
                NodeKind::Branch {
                    condition: MoveCondition::AttackInfliction {
                        query: InflictionQuery::CurrentHitlag,
                        mask: InflictionMask::Hit,
                    },
                    then_nodes: vec![fixture::wait(2, 1.0)],
                    else_nodes: Vec::new(),
                },
            )],
        );
        let errors = compile("mario", "attack11", &graph(authored_sound))
            .expect_err("authored sound-track infliction condition must fail closed");
        assert!(errors.iter().any(|error| {
            error
                .reason
                .contains("AttackModule infliction conditions are supported only on the Game track")
        }));
    }

    #[test]
    fn attack_infliction_statement_attributes_cannot_authorize_editable_branches() {
        const BODY: &str = "if AttackModule::is_infliction_status(agent.module_accessor, *COLLISION_KIND_MASK_HIT) {\nwait(agent.lua_state_agent, 1.0);\n} else {\nwait(agent.lua_state_agent, 2.0);\n}";
        for attribute in [
            "#[cfg(any())]",
            "#[cfg(any())]\n\n// still attached",
            "#[cfg(\nany()\n)]",
        ] {
            let source = format!(
                "unsafe extern \"C\" fn game_test(agent: &mut L2CAgentBase) {{\n{attribute}\n{BODY}\n}}\n"
            );
            let original = crate::acmd::parse_acmd_script(&source);
            let imported = import_for_fighter("mario", Track::Game, &original);
            assert!(
                imported
                    .nodes
                    .iter()
                    .all(|node| !matches!(node.kind, NodeKind::Branch { .. })),
                "{attribute}"
            );
            let compiled = compile("mario", "test", &graph(imported)).unwrap();
            assert_json_eq!(&original, &compiled[0].1);
        }
    }

    #[test]
    fn noncanonical_infliction_forms_remain_opaque() {
        let cases = [
            "if smash::app::lua_bind::AttackModule::is_infliction(agent.module_accessor, *COLLISION_KIND_MASK_HIT) {",
            "if AttackModule::is_infliction(agent.module_accessor, 2) {",
            "if AttackModule::is_infliction(agent.module_accessor, *COLLISION_KIND_MASK_HS) {",
            "if AttackModule::is_infliction(agent.module_accessor, *COLLISION_KIND_MASK_HIT, false) {",
            "if AttackModule::is_infliction_status(module_accessor, *COLLISION_KIND_MASK_HIT) {",
            "if AttackModule::get_inflict_status(agent.module_accessor) == 2 {",
            "if #[inline] AttackModule::is_infliction(agent.module_accessor, *COLLISION_KIND_MASK_HIT) {",
        ];
        for header in cases {
            assert!(
                import_condition(Some("mario"), header).is_none(),
                "noncanonical infliction source was admitted: {header}"
            );

            let source = AcmdScript {
                stmts: vec![
                    AcmdStmt::RawBlock {
                        header: header.into(),
                        body: vec![AcmdStmt::Wait(1.0)],
                    },
                    AcmdStmt::RawBlock {
                        header: "else {".into(),
                        body: vec![AcmdStmt::Wait(2.0)],
                    },
                ],
            };
            let imported = import_for_fighter("mario", Track::Game, &source);
            assert!(imported
                .nodes
                .iter()
                .all(|node| fixture::untyped_condition(&node.kind)));
            if matches!(imported.nodes[0].kind, NodeKind::Preserved { .. }) {
                assert_json_eq!(&imported.baseline, &source.stmts);
            }
            if !header.contains("#[") {
                let compiled = compile("mario", "attack11", &graph(imported)).unwrap();
                assert_json_eq!(&compiled[0].1, &source);
                let emitted = crate::acmd::preview_game_fn(&compiled[0].1, "attack11");
                let parsed = crate::acmd::parse_acmd_script(&emitted);
                assert_json_eq!(parsed, source);
            }
        }
    }

    #[test]
    fn native_lr_negative_is_exact_source_only_condition_on_game_effect_expression() {
        const HEADER: &str = "if get_value_float(agent.lua_state_agent, *SO_VAR_FLOAT_LR) < 0.0 {";
        let condition = MoveCondition::NativeLrNegative;
        assert_eq!(
            serde_json::to_value(import_atomic_condition("mario", HEADER).unwrap()).unwrap(),
            serde_json::json!("native_lr_negative")
        );
        assert_eq!(
            crate::move_nodes::compound_conditions::header("mario", &condition).unwrap(),
            HEADER
        );

        let compound = MoveCondition::Not(Box::new(MoveCondition::Group(Box::new(
            MoveCondition::And {
                left: Box::new(MoveCondition::NativeLrNegative),
                right: Box::new(MoveCondition::Group(Box::new(MoveCondition::Or {
                    left: Box::new(MoveCondition::Grounded),
                    right: Box::new(MoveCondition::Not(Box::new(MoveCondition::Group(
                        Box::new(MoveCondition::Airborne),
                    )))),
                }))),
            },
        ))));
        let compound_header =
            crate::move_nodes::compound_conditions::header("mario", &compound).unwrap();
        let imported_compound =
            crate::move_nodes::compound_conditions::import("mario", &compound_header)
                .expect("All/Any/Not/Group may compose the one fixed source leaf");
        assert_json_eq!(imported_compound, &compound);
        assert!(compound_header
            .contains("get_value_float(agent.lua_state_agent, *SO_VAR_FLOAT_LR) < 0.0"));

        for (domain, function_name) in [
            (Track::Game, "game_lr_condition"),
            (Track::Effect, "effect_lr_condition"),
            (Track::Expression, "expression_lr_condition"),
        ] {
            for branch_header in [HEADER.to_owned(), compound_header.clone()] {
                let source = format!(
                    "unsafe extern \"C\" fn {function_name}(agent: &mut L2CAgentBase) {{\n    {branch_header}\n        frame(agent.lua_state_agent, 3.0);\n    }} else {{\n        frame(agent.lua_state_agent, 7.0);\n    }}\n}}\n"
                );
                let original = match domain {
                    Track::Game => crate::acmd::parse_acmd_script(&source),
                    Track::Effect => {
                        let effect = crate::acmd::parse_effect_script(&source);
                        AcmdScript {
                            stmts: effect
                                .stmts
                                .iter()
                                .map(super::super::effects::effect_stmt_to_acmd)
                                .collect(),
                        }
                    }
                    Track::Expression => crate::acmd::parse_expression_script(&source),
                    _ => unreachable!(),
                };
                let imported = import_for_fighter("mario", domain, &original);
                assert!(
                    matches!(
                        imported.nodes.as_slice(),
                        [MoveNode {
                            kind: NodeKind::Branch { then_nodes, else_nodes, .. },
                            ..
                        }] if matches!(then_nodes.as_slice(), [MoveNode { kind: NodeKind::Frame { frame: 3.0 }, .. }])
                            && matches!(else_nodes.as_slice(), [MoveNode { kind: NodeKind::Frame { frame: 7.0 }, .. }])
                    ),
                    "{domain:?} must retain both nonempty branch arms for {branch_header}"
                );
                assert!(
                    matches!(
                        &imported.nodes[0].kind,
                        NodeKind::Branch {
                            condition: MoveCondition::NativeLrNegative,
                            ..
                        }
                    ) || matches!(
                        &imported.nodes[0].kind,
                        NodeKind::Branch {
                            condition: MoveCondition::Not(_),
                            ..
                        }
                    )
                );
                let compiled = compile("mario", function_name, &graph(imported)).unwrap();
                let emitted = super::preview(domain, &compiled[0].1, function_name);
                syn::parse_file(&emitted).expect("generated source remains valid Rust");
                let reparsed = match domain {
                    Track::Game => crate::acmd::parse_acmd_script(&emitted),
                    Track::Effect => {
                        let effect = crate::acmd::parse_effect_script(&emitted);
                        AcmdScript {
                            stmts: effect
                                .stmts
                                .iter()
                                .map(super::super::effects::effect_stmt_to_acmd)
                                .collect(),
                        }
                    }
                    Track::Expression => crate::acmd::parse_expression_script(&emitted),
                    _ => unreachable!(),
                };
                assert_json_eq!(
                    reparsed,
                    original,
                    "{domain:?} source order or Else association changed"
                );
            }
        }

        let nested_source = AcmdScript {
            stmts: vec![
                AcmdStmt::RawBlock {
                    header: "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND {".into(),
                    body: vec![AcmdStmt::RawBlock {
                        header: HEADER.into(),
                        body: vec![AcmdStmt::Wait(1.0)],
                    }],
                },
                AcmdStmt::RawBlock {
                    header: "else {".into(),
                    body: vec![AcmdStmt::Wait(2.0)],
                },
            ],
        };
        let nested = import_for_fighter("mario", Track::Game, &nested_source);
        assert!(matches!(
            nested.nodes.as_slice(),
            [MoveNode {
                kind: NodeKind::Branch { then_nodes, else_nodes, .. },
                ..
            }] if matches!(then_nodes.as_slice(), [MoveNode {
                kind: NodeKind::Branch { condition: MoveCondition::NativeLrNegative, .. },
                ..
            }]) && matches!(else_nodes.as_slice(), [MoveNode { kind: NodeKind::Wait { frames: 2.0 }, .. }])
        ));
        let nested_compiled = compile("mario", "nested_lr", &graph(nested)).unwrap();
        assert_json_eq!(
            &nested_compiled[0].1,
            nested_source,
            "nested supported outer branch retains the LR child and Else order"
        );

        let adjacent_same_headers = AcmdScript {
            stmts: vec![
                AcmdStmt::RawBlock {
                    header: HEADER.into(),
                    body: vec![AcmdStmt::Wait(1.0)],
                },
                AcmdStmt::RawBlock {
                    header: "else {".into(),
                    body: vec![AcmdStmt::Wait(2.0)],
                },
                AcmdStmt::RawBlock {
                    header: HEADER.into(),
                    body: vec![AcmdStmt::Wait(3.0)],
                },
                AcmdStmt::RawBlock {
                    header: "else {".into(),
                    body: vec![AcmdStmt::Wait(4.0)],
                },
                AcmdStmt::Wait(5.0),
            ],
        };
        let adjacent = import_for_fighter("mario", Track::Game, &adjacent_same_headers);
        assert!(matches!(
            adjacent.nodes.as_slice(),
            [MoveNode { kind: NodeKind::Branch { then_nodes: then_a, else_nodes: else_a, .. }, .. },
             MoveNode { kind: NodeKind::Branch { then_nodes: then_b, else_nodes: else_b, .. }, .. },
             MoveNode { kind: NodeKind::Wait { frames: 5.0 }, .. }]
                if matches!(then_a.as_slice(), [MoveNode { kind: NodeKind::Wait { frames: 1.0 }, .. }])
                    && matches!(else_a.as_slice(), [MoveNode { kind: NodeKind::Wait { frames: 2.0 }, .. }])
                    && matches!(then_b.as_slice(), [MoveNode { kind: NodeKind::Wait { frames: 3.0 }, .. }])
                    && matches!(else_b.as_slice(), [MoveNode { kind: NodeKind::Wait { frames: 4.0 }, .. }])
        ));
        let adjacent_compiled = compile("mario", "adjacent_lr", &graph(adjacent)).unwrap();
        assert_json_eq!(
            &adjacent_compiled[0].1,
            adjacent_same_headers,
            "repeated identical headers must keep their own Else arms and following sibling"
        );
    }

    #[test]
    fn native_lr_negative_stays_raw_for_sound_unsupported_shapes_and_bad_functions() {
        const HEADER: &str = "if get_value_float(agent.lua_state_agent, *SO_VAR_FLOAT_LR) < 0.0 {";
        let unsupported = [
            HEADER.replace("< 0.0", "<= 0.0"),
            HEADER.replace("< 0.0", "< 1.0"),
            HEADER.replace("< 0.0", "> 0.0"),
            HEADER.replace("< 0.0", "== 0.0"),
            HEADER.replace("agent.lua_state_agent", "other.lua_state_agent"),
            HEADER.replace("agent.lua_state_agent", "agent.module_accessor"),
            HEADER.replace("*SO_VAR_FLOAT_LR", "*SO_VAR_FLOAT_LR_SUFFIX"),
            HEADER.replace("*SO_VAR_FLOAT_LR", "SO_VAR_FLOAT_LR"),
            HEADER.replace("get_value_float(", "sv_animcmd::get_value_float("),
            HEADER.replace(") < 0.0", ") as f32 < 0.0"),
            HEADER.replace("< 0.0", "< -0.0"),
            HEADER.replace(") < 0.0", ", 1) < 0.0"),
            HEADER
                .replace("if get_value_float(", "if (get_value_float(")
                .replace(") < 0.0", ")) < 0.0"),
            HEADER.replace("if ", "#[cfg(any())]\nif "),
            "if get_value_float(agent.lua_state_agent, *SO_VAR_FLOAT_LR) < 0.0 && arbitrary(agent) {".into(),
            "if get_value_float(agent.lua_state_agent, *SO_VAR_FLOAT_LR) < 0.0 { // suffix".into(),
        ];
        for header in unsupported {
            assert!(
                import_atomic_condition("mario", &header).is_none(),
                "noncanonical candidate admitted: {header}"
            );

            let opaque = AcmdScript {
                stmts: vec![
                    AcmdStmt::RawBlock {
                        header: header.clone(),
                        body: vec![AcmdStmt::Wait(1.0)],
                    },
                    AcmdStmt::RawBlock {
                        header: "else {".into(),
                        body: vec![AcmdStmt::Wait(2.0)],
                    },
                ],
            };
            for track in [Track::Game, Track::Effect, Track::Expression] {
                let imported = import_for_fighter("mario", track, &opaque);
                assert!(imported
                    .nodes
                    .iter()
                    .all(|node| fixture::untyped_condition(&node.kind)));
                let source = super::preview(track, &opaque, "opaque_lr");
                let original_graph = graph(imported);
                let before = serde_json::to_value(&original_graph).unwrap();
                let result = compile("mario", "opaque_lr", &original_graph);
                match syn::parse_file(&source) {
                    Ok(_) => {
                        let roundtrip = result.unwrap_or_else(|errors| {
                            panic!(
                                "valid opaque source rejected on {track:?}: {header}: {errors:?}"
                            )
                        });
                        assert_json_eq!(
                            &roundtrip[0].1,
                            &opaque,
                            "noncanonical header changed on {track:?}: {header}"
                        );
                    }
                    Err(_) => {
                        let errors =
                            result.expect_err("malformed native source must not be repaired");
                        assert!(
                            errors.iter().all(|error| error
                                .reason
                                .starts_with("generated ACMD failed Rust parsing:")),
                            "{track:?}: {header}: {errors:?}"
                        );
                    }
                }
                assert_eq!(serde_json::to_value(original_graph).unwrap(), before);
            }
        }

        let exact = AcmdStmt::RawBlock {
            header: HEADER.into(),
            body: vec![AcmdStmt::Wait(1.0)],
        };
        let source = AcmdScript {
            stmts: vec![
                exact.clone(),
                AcmdStmt::RawBlock {
                    header: "else {".into(),
                    body: vec![AcmdStmt::Wait(2.0)],
                },
            ],
        };
        let sound = import_for_fighter("mario", Track::Sound, &source);
        assert!(sound
            .nodes
            .iter()
            .all(|node| matches!(node.kind, NodeKind::Preserved { .. })));
        let compiled = compile("mario", "sound_lr_negative", &graph(sound)).unwrap();
        assert_json_eq!(
            &compiled[0].1,
            &source,
            "Sound must preserve both LR-sign source and its Else as Raw source"
        );

        let compound_sound_source = AcmdScript {
            stmts: vec![AcmdStmt::RawBlock {
                header: crate::move_nodes::compound_conditions::header(
                    "mario",
                    &MoveCondition::And {
                        left: Box::new(MoveCondition::NativeLrNegative),
                        right: Box::new(MoveCondition::Grounded),
                    },
                )
                .unwrap(),
                body: vec![AcmdStmt::Wait(1.0)],
            }],
        };
        let compound_sound = import_for_fighter("mario", Track::Sound, &compound_sound_source);
        assert!(compound_sound
            .nodes
            .iter()
            .all(|node| matches!(node.kind, NodeKind::Preserved { .. })));
        let compound_sound_compiled =
            compile("mario", "compound_sound_lr", &graph(compound_sound)).unwrap();
        assert_json_eq!(
            &compound_sound_compiled[0].1,
            compound_sound_source,
            "Sound rejects LR-sign leaves nested in canonical compounds without rewriting them"
        );
        let authored_sound = graph(fixture::track(
            Track::Sound,
            vec![fixture::node(
                1,
                NodeKind::Branch {
                    condition: MoveCondition::Not(Box::new(MoveCondition::NativeLrNegative)),
                    then_nodes: vec![],
                    else_nodes: vec![],
                },
            )],
        ));
        let sound_error = compile("mario", "authored_sound_lr", &authored_sound)
            .expect_err("a saved Sound graph cannot introduce this source leaf");
        assert!(sound_error.iter().any(|error| {
            error
                .reason
                .contains("supported only on Game, Effect, and Expression tracks")
        }));

        let mixed_sound_error = compile(
            "mario",
            "sound_mixed_lr_motion",
            &graph(fixture::track(
                Track::Sound,
                vec![fixture::node(
                    1,
                    NodeKind::Branch {
                        condition: MoveCondition::And {
                            left: Box::new(MoveCondition::NativeLrNegative),
                            right: Box::new(MoveCondition::MotionChanging),
                        },
                        then_nodes: Vec::new(),
                        else_nodes: Vec::new(),
                    },
                )],
            )),
        )
        .expect_err("Sound rejects both leaves in the compound condition");
        assert!(mixed_sound_error.iter().any(|error| {
            error
                .reason
                .contains("supported only on Game, Effect, and Expression tracks")
        }));

        let mixed_expression_error = compile(
            "mario",
            "expression_mixed_lr_motion",
            &graph(fixture::track(
                Track::Expression,
                vec![fixture::node(
                    1,
                    NodeKind::Branch {
                        condition: MoveCondition::And {
                            left: Box::new(MoveCondition::NativeLrNegative),
                            right: Box::new(MoveCondition::MotionChanging),
                        },
                        then_nodes: Vec::new(),
                        else_nodes: Vec::new(),
                    },
                )],
            )),
        )
        .expect_err("Expression admits LR source but not MotionModule source");
        assert!(mixed_expression_error
            .iter()
            .any(|error| error.reason.contains(
                "MotionModule::is_changing conditions are supported only on the Game track"
            )));
        assert!(mixed_expression_error.iter().all(|error| {
            !error
                .reason
                .contains("native LR-sign source conditions are supported only")
        }));

        let unsupported_outer = AcmdScript {
            stmts: vec![AcmdStmt::RawBlock {
                header: "if unreviewed_outer(agent) {".into(),
                body: vec![exact.clone()],
            }],
        };
        let imported = import_for_fighter("mario", Track::Game, &unsupported_outer);
        assert!(matches!(
            imported.nodes.as_slice(),
            [MoveNode { kind: NodeKind::Branch { condition: MoveCondition::Source { header }, then_nodes, .. }, .. }]
                if header == "if unreviewed_outer(agent) {"
                    && matches!(then_nodes.as_slice(), [MoveNode { kind: NodeKind::Branch { condition: MoveCondition::NativeLrNegative, .. }, .. }])
        ));
        let unchanged = compile("mario", "nested_unsupported", &graph(imported)).unwrap();
        assert_json_eq!(
            &unchanged[0].1,
            &unsupported_outer,
            "a recognized child under an unsupported outer header keeps the whole source tree"
        );

        let empty_else = AcmdScript {
            stmts: vec![
                exact.clone(),
                AcmdStmt::RawBlock {
                    header: "else {".into(),
                    body: vec![],
                },
            ],
        };
        let imported = import_for_fighter("mario", Track::Game, &empty_else);
        assert!(imported
            .nodes
            .iter()
            .all(|node| matches!(node.kind, NodeKind::Preserved { .. })));
        let roundtrip = compile("mario", "empty_else", &graph(imported)).unwrap();
        assert_json_eq!(&roundtrip[0].1, empty_else);

        let else_if = AcmdScript {
            stmts: vec![
                exact,
                AcmdStmt::RawBlock {
                    header: "else if unreviewed(agent) {".into(),
                    body: vec![AcmdStmt::Wait(4.0)],
                },
            ],
        };
        let imported = import_for_fighter("mario", Track::Expression, &else_if);
        assert!(imported
            .nodes
            .iter()
            .all(|node| matches!(node.kind, NodeKind::Preserved { .. })));
        let roundtrip = compile("mario", "else_if", &graph(imported)).unwrap();
        assert_json_eq!(&roundtrip[0].1, else_if);

        let malformed = r#"unsafe extern "C" fn game_lr_broken(agent: &mut L2CAgentBase) {
    if get_value_float(agent.lua_state_agent, *SO_VAR_FLOAT_LR) < 0.0 {
        frame(agent.lua_state_agent, 1.0);
    }
    let broken = ;
}
"#;
        let parsed = crate::acmd::parse_acmd_script(malformed);
        assert!(parsed
            .stmts
            .iter()
            .any(|stmt| matches!(stmt, AcmdStmt::Raw(line) if line.contains("let broken = ;"))));
        assert!(syn::parse_file(&crate::acmd::preview_game_fn(&parsed, "lr_broken")).is_err());
        let imported = import_for_fighter("mario", Track::Game, &parsed);
        let errors = compile("mario", "lr_broken", &graph(imported))
            .expect_err("malformed sibling must fail closed");
        assert!(
            errors.iter().all(|error| error
                .reason
                .starts_with("generated ACMD failed Rust parsing:")),
            "{errors:?}"
        );
    }

    #[test]
    fn expression_wind_families_parse_import_compile_export_and_reparse_under_guard() {
        let source = r#"unsafe extern "C" fn expression_specialhihold(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 1.0);
    if macros::is_excute(agent) {
        macros::AREA_WIND_2ND_RAD(agent, 1, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0);
        macros::AREA_WIND_2ND_RAD_arg9(agent, 2, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0);
        macros::AREA_WIND_2ND_arg10(agent, 3, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0);
        AreaModule::erase_wind(agent.module_accessor, 1);
        AreaModule::erase_wind(agent.module_accessor, 2);
        AreaModule::erase_wind(agent.module_accessor, 3);
    }
}
"#;
        let parsed = crate::acmd::parse_expression_script(source);
        assert!(matches!(parsed.stmts.first(), Some(AcmdStmt::Frame(frame)) if *frame == 1.0));
        assert!(
            matches!(parsed.stmts.get(1), Some(AcmdStmt::Excute(commands)) if commands.len() == 6)
        );

        let imported = import_for_fighter("mario", Track::Expression, &parsed);
        let execute = imported
            .nodes
            .iter()
            .find_map(|node| match &node.kind {
                NodeKind::Execute { commands } => Some(commands),
                _ => None,
            })
            .expect("guarded wind actions should be editable as one execute block");
        assert_eq!(execute.len(), 6);
        assert!(execute[..3]
            .iter()
            .all(|command| matches!(command, ExcuteStmt::Wind(wind) if wind.is_valid() && wind.has_macro_wrapper())));
        assert!(execute[3..]
            .iter()
            .all(|command| matches!(command, ExcuteStmt::EraseWind(_))));

        let compiled = compile("mario", "specialhihold", &graph(imported)).unwrap();
        let exported = preview(Track::Expression, &compiled[0].1, "specialhihold");
        let reparsed = crate::acmd::parse_expression_script(&exported);
        assert_json_eq!(
            &parsed,
            &reparsed,
            "frame ordering, execute guard, wind payloads, and erase IDs should survive export"
        );
    }

    #[test]
    fn expression_wind_unwrapped_wrong_track_and_invalid_shapes_stay_immutable() {
        let unwrapped_source = r#"unsafe extern "C" fn expression_specialhihold(agent: &mut L2CAgentBase) {
    macros::AREA_WIND_2ND_RAD(agent, 1, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0);
}
"#;
        let unwrapped = crate::acmd::parse_expression_script(unwrapped_source);
        let imported = import_for_fighter("mario", Track::Expression, &unwrapped);
        assert!(matches!(imported.nodes[0].kind, NodeKind::Preserved { .. }));
        assert!(matches!(unwrapped.stmts[0], AcmdStmt::Raw(_)));

        let plain_rectangle_source = r#"unsafe extern "C" fn expression_specialhihold(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::AREA_WIND_2ND(agent, 1, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0);
    }
}
"#;
        let plain_rectangle = crate::acmd::parse_expression_script(plain_rectangle_source);
        let imported = import_for_fighter("mario", Track::Expression, &plain_rectangle);
        assert!(matches!(imported.nodes[0].kind, NodeKind::Preserved { .. }));
        roundtrip(Track::Expression, plain_rectangle.stmts.clone());

        let wind = ExcuteStmt::Wind(crate::data::WindboxData {
            command: "AREA_WIND_2ND_RAD".into(),
            args: vec![1.0; 8],
        });
        assert!(command_matches_track(Track::Expression, &wind));
        assert!(!command_matches_track(Track::Sound, &wind));
        assert!(!bare_command_matches_track(Track::Expression, &wind));
        let sound_import = import(
            Track::Sound,
            &AcmdScript {
                stmts: vec![AcmdStmt::Excute(vec![wind.clone()])],
            },
        );
        assert!(matches!(
            sound_import.nodes[0].kind,
            NodeKind::Preserved { .. }
        ));

        for malformed in [
            crate::data::WindboxData {
                command: "unregistered_wind".into(),
                args: vec![1.0; 8],
            },
            crate::data::WindboxData {
                command: "AREA_WIND_2ND_RAD".into(),
                args: vec![1.0; 9],
            },
            crate::data::WindboxData {
                command: "AREA_WIND_2ND".into(),
                args: vec![1.0; 9],
            },
            crate::data::WindboxData {
                command: "AREA_WIND_2ND_RAD".into(),
                args: vec![f32::NAN; 8],
            },
        ] {
            assert!(validate_command(None, &ExcuteStmt::Wind(malformed)).is_err());
        }
    }

    #[test]
    fn expression_hurtbox_states_import_compile_preview_and_reparse_exactly() {
        let source = r#"unsafe extern "C" fn expression_attack11(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 2.0);
    if macros::is_excute(agent) {
        macros::HIT_NODE(agent, Hash40::new("kneer"), *HIT_STATUS_NORMAL);
        macros::HIT_NODE(agent, Hash40::new("armr"), *HIT_STATUS_INVINCIBLE);
        macros::HIT_NO(agent, 8, *HIT_STATUS_XLU);
        macros::WHOLE_HIT(agent, *HIT_STATUS_OFF);
        macros::HIT_RESET_ALL(agent);
        macros::HIT_NO(agent, 3, 3);
        macros::HIT_NODE(agent, Hash40::new("head"), *HIT_STATUS_UNKNOWN);
        macros::HIT_NODE(agent, Hash40::new("footr"), *HIT_STATUS_XLU, 0);
        HitModule::set_status_joint(agent.module_accessor, Hash40::new("shoulderr"), HitStatus(*HIT_STATUS_XLU), 0);
        HitModule::set_status_joint(agent.module_accessor, Hash40::new("shoulderl"), HitStatus(*HIT_STATUS_XLU), 1);
        custom_expression_control(agent.module_accessor, 7);
    }
}
"#;
        let parsed = crate::acmd::parse_expression_script(source);
        let imported = import_for_fighter("mario", Track::Expression, &parsed);
        assert_json_eq!(&imported.baseline, vec![parsed.stmts[1].clone()]);
        assert!(
            matches!(
                imported.nodes.as_slice(),
                [
                    MoveNode { kind: NodeKind::Frame { frame }, .. },
                    MoveNode { kind: NodeKind::MixedExecute { commands }, .. }
                ] if *frame == 2.0
                    && matches!(commands.as_slice(), [
                        MoveCommand::Editable(ExcuteStmt::HitStatus {
                            target: crate::data::HurtTarget::Bone(bone), status: normal
                        }),
                        MoveCommand::Editable(ExcuteStmt::HitStatus {
                            target: crate::data::HurtTarget::Bone(invincible_bone), status: invincible
                        }),
                        MoveCommand::Editable(ExcuteStmt::HitStatus {
                            target: crate::data::HurtTarget::Group(group), status: xlu
                        }),
                        MoveCommand::Editable(ExcuteStmt::HitStatus {
                            target: crate::data::HurtTarget::Whole, status: off
                        }),
                        MoveCommand::Editable(ExcuteStmt::HitResetAll),
                        MoveCommand::Editable(ExcuteStmt::HitStatus {
                            target: crate::data::HurtTarget::Group(3), status: numeric
                        }),
                        MoveCommand::Preserved(ExcuteStmt::HitStatus { status: invalid_status, .. }),
                        MoveCommand::Preserved(ExcuteStmt::Raw(malformed_arity)),
                        MoveCommand::Preserved(ExcuteStmt::Raw(native_zero_tail)),
                        MoveCommand::Preserved(ExcuteStmt::Raw(native_nonzero_tail)),
                        MoveCommand::Preserved(ExcuteStmt::Raw(raw)),
                    ] if bone == "kneer"
                        && normal == "HIT_STATUS_NORMAL"
                        && invincible_bone == "armr"
                        && invincible == "HIT_STATUS_INVINCIBLE"
                        && *group == 8
                        && xlu == "HIT_STATUS_XLU"
                        && off == "HIT_STATUS_OFF"
                        && numeric == "3"
                        && invalid_status == "HIT_STATUS_UNKNOWN"
                        && malformed_arity == "macros::HIT_NODE(agent, Hash40::new(\"footr\"), *HIT_STATUS_XLU, 0);"
                        && native_zero_tail == "HitModule::set_status_joint(agent.module_accessor, Hash40::new(\"shoulderr\"), HitStatus(*HIT_STATUS_XLU), 0);"
                        && native_nonzero_tail == "HitModule::set_status_joint(agent.module_accessor, Hash40::new(\"shoulderl\"), HitStatus(*HIT_STATUS_XLU), 1);"
                        && raw == "custom_expression_control(agent.module_accessor, 7);")
            ),
            "measured macro targets/statuses should be editable while invalid, malformed, and native calls stay preserved"
        );

        let compiled = compile("mario", "expression_attack11", &graph(imported)).unwrap();
        assert_eq!(compiled[0].0, Track::Expression);
        assert_json_eq!(
            &compiled[0].1,
            &parsed,
            "compilation should retain the exact typed and preserved payloads"
        );
        let preview = preview(Track::Expression, &compiled[0].1, "expression_attack11");
        let reparsed = crate::acmd::parse_expression_script(&preview);
        assert_json_eq!(
            &reparsed,
            &parsed,
            "Expression source preview should preserve targets, statuses, reset, and unknown payload"
        );
    }

    #[test]
    fn expression_hurtbox_states_stay_preserved_on_other_tracks() {
        let parsed = crate::acmd::parse_expression_script(
            r#"unsafe extern "C" fn expression_attack11(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::HIT_NODE(agent, Hash40::new("kneer"), *HIT_STATUS_XLU);
        macros::HIT_RESET_ALL(agent);
    }
}
"#,
        );
        let sound_import = import_for_fighter("mario", Track::Sound, &parsed);
        assert!(matches!(
            sound_import.nodes.as_slice(),
            [MoveNode {
                kind: NodeKind::Preserved { .. },
                ..
            }]
        ));

        let bare = crate::acmd::parse_expression_script(
            r#"unsafe extern "C" fn expression_attack11(agent: &mut L2CAgentBase) {
    macros::HIT_NODE(agent, Hash40::new("kneer"), *HIT_STATUS_XLU);
    macros::HIT_RESET_ALL(agent);
}
"#,
        );
        assert!(matches!(
            import_for_fighter("mario", Track::Expression, &bare)
                .nodes
                .as_slice(),
            [
                MoveNode {
                    kind: NodeKind::Preserved { .. },
                    ..
                },
                MoveNode {
                    kind: NodeKind::Preserved { .. },
                    ..
                }
            ]
        ));

        let hurtbox = ExcuteStmt::HitStatus {
            target: crate::data::HurtTarget::Whole,
            status: "HIT_STATUS_NORMAL".into(),
        };
        assert!(command_matches_track(Track::Expression, &hurtbox));
        assert!(!command_matches_track(Track::Sound, &hurtbox));
        assert!(!command_matches_track(Track::Effect, &hurtbox));
        assert!(command_matches_track(
            Track::Expression,
            &ExcuteStmt::HitResetAll
        ));
        assert!(!command_matches_track(
            Track::Sound,
            &ExcuteStmt::HitResetAll
        ));
        assert!(!command_matches_track(
            Track::Effect,
            &ExcuteStmt::HitResetAll
        ));
    }

    fn work_flag(flag: &str) -> ExcuteStmt {
        ExcuteStmt::WorkFlag(WorkFlagCall {
            action: WorkFlagAction::On,
            flag: flag.into(),
        })
    }

    fn attack() -> ExcuteStmt {
        ExcuteStmt::Attack(AttackCall {
            func: "ATTACK".into(),
            id: 0,
            part: 0,
            bone_name: "top".into(),
            damage: 5.0,
            angle: 45,
            kb_scaling: 100,
            fkb: 0,
            kb_base: 30,
            size: 3.0,
            offset_x: 0.0,
            offset_y: 5.0,
            offset_z: 0.0,
            capsule_end: None,
            hitlag_mult: 1.0,
            sdi_mult: 1.0,
            setoff_kind: "ATTACK_SETOFF_KIND_ON".into(),
            lr_check: "ATTACK_LR_CHECK_F".into(),
            is_clang: true,
            is_add_attack: 0,
            hitbox_attr: 0.0,
            ground_or_air: 0,
            is_mtk: false,
            is_shield_disable: false,
            is_reflectable: false,
            is_absorbable: false,
            is_landing_attack: false,
            situation_mask: "COLLISION_SITUATION_MASK_GA".into(),
            category_mask: "COLLISION_CATEGORY_MASK_ALL".into(),
            part_mask: "COLLISION_PART_MASK_ALL".into(),
            no_finish_camera: false,
            collision_attr: "collision_attr_normal".into(),
            sound_level: "ATTACK_SOUND_LEVEL_S".into(),
            sound_attr: "COLLISION_SOUND_ATTR_PUNCH".into(),
            attack_region: "ATTACK_REGION_PUNCH".into(),
        })
    }

    #[test]
    fn imported_game_script_roundtrips_json_exactly() {
        roundtrip(
            Track::Game,
            vec![
                AcmdStmt::Frame(3.0),
                AcmdStmt::Excute(vec![
                    ExcuteStmt::ClearAll,
                    work_flag("*FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO"),
                ]),
                AcmdStmt::Wait(2.5),
                AcmdStmt::MotionRate(0.8),
                AcmdStmt::Raw("custom_call(agent);".into()),
            ],
        );
    }

    #[test]
    fn measured_motion_rate_and_bare_sound_roundtrip_as_typed_nodes() {
        roundtrip(
            Track::Game,
            vec![AcmdStmt::MotionRate(0.5), AcmdStmt::Frame(4.0)],
        );

        let bare_sound = AcmdStmt::Bare(Box::new(ExcuteStmt::Sound(SoundCall {
            func: "PLAY_SE".into(),
            sounds: vec!["se_common_swing_01".into()],
            tail: None,
        })));
        let imported = import(
            Track::Sound,
            &AcmdScript {
                stmts: vec![bare_sound.clone()],
            },
        );
        assert!(matches!(
            imported.nodes[0].kind,
            NodeKind::BareCommand { .. }
        ));
        roundtrip(Track::Sound, vec![bare_sound]);
    }

    #[test]
    fn motion_rate_must_be_finite_and_positive() {
        for rate in [0.0, -0.5, f32::NAN, f32::INFINITY] {
            let imported = import(
                Track::Game,
                &AcmdScript {
                    stmts: vec![AcmdStmt::MotionRate(rate)],
                },
            );
            assert!(matches!(imported.nodes[0].kind, NodeKind::Preserved { .. }));

            let authored = fixture::track(
                Track::Game,
                vec![fixture::node(1, NodeKind::MotionRate { rate })],
            );
            let errors = compile("mario", "attack11", &graph(authored)).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|error| error.reason.contains("motion rate must be finite")),
                "rate {rate:?}: {errors:?}"
            );
        }

        let sound_track = fixture::track(
            Track::Sound,
            vec![fixture::node(1, NodeKind::MotionRate { rate: 0.5 })],
        );
        let errors = compile("mario", "attack11", &graph(sound_track)).unwrap_err();
        assert!(errors
            .iter()
            .any(|error| error.reason.contains("only valid on the Game track")));
    }

    #[test]
    fn bare_collision_commands_stay_preserved_and_cannot_be_authored() {
        let source = AcmdStmt::Bare(Box::new(attack()));
        let imported = import(
            Track::Game,
            &AcmdScript {
                stmts: vec![source.clone()],
            },
        );
        assert!(matches!(imported.nodes[0].kind, NodeKind::Preserved { .. }));
        roundtrip(Track::Game, vec![source]);

        let authored = fixture::track(
            Track::Game,
            vec![fixture::node(
                1,
                NodeKind::BareCommand {
                    command: Box::new(attack()),
                },
            )],
        );
        let errors = compile("mario", "attack11", &graph(authored)).unwrap_err();
        assert!(errors
            .iter()
            .any(|error| error.reason.contains("bare command is not supported")));
    }

    #[test]
    fn wait_loop_clear_is_typed_at_the_root_and_preserved_when_nested() {
        let source = AcmdStmt::WaitLoopClear;
        let imported = import(
            Track::Game,
            &AcmdScript {
                stmts: vec![source.clone()],
            },
        );
        assert!(matches!(
            imported.nodes[0].kind,
            NodeKind::NativeWaitLoopClear
        ));
        roundtrip(Track::Game, vec![source.clone()]);
        let nested = AcmdStmt::Loop {
            motion_loop: crate::data::MotionLoop::No,
            count: 2,
            body: vec![source],
        };
        let imported = import(
            Track::Game,
            &AcmdScript {
                stmts: vec![nested.clone()],
            },
        );
        let NodeKind::Repeat { body, .. } = &imported.nodes[0].kind else {
            panic!()
        };
        assert!(matches!(body[0].kind, NodeKind::Preserved { .. }));
        roundtrip(Track::Game, vec![nested]);
    }

    #[test]
    fn nested_bounded_loops_roundtrip_and_compile() {
        roundtrip(
            Track::Game,
            vec![AcmdStmt::Loop {
                motion_loop: crate::data::MotionLoop::No,
                count: 3,
                body: vec![
                    AcmdStmt::Frame(1.0),
                    AcmdStmt::Loop {
                        motion_loop: crate::data::MotionLoop::No,
                        count: 2,
                        body: vec![AcmdStmt::Excute(vec![ExcuteStmt::ClearAll])],
                    },
                ],
            }],
        );
    }

    #[test]
    fn raw_commands_and_unmodeled_blocks_are_preserved_whole() {
        roundtrip(
            Track::Game,
            vec![
                AcmdStmt::Excute(vec![
                    ExcuteStmt::ClearAll,
                    ExcuteStmt::Raw("custom_call(agent);".into()),
                ]),
                AcmdStmt::RawBlock {
                    header: "if WorkModule::is_flag(agent.module_accessor, *FLAG) {".into(),
                    body: vec![
                        AcmdStmt::Wait(1.0),
                        AcmdStmt::Excute(vec![ExcuteStmt::ClearAll]),
                    ],
                },
            ],
        );
    }

    #[test]
    fn authored_raw_is_rejected_but_imported_raw_is_immutable() {
        let mut track = fixture::track(
            Track::Game,
            vec![fixture::execute(1, vec![ExcuteStmt::Raw("x();".into())])],
        );
        assert!(compile("mario", "attack11", &graph(track.clone())).is_err());
        let original = AcmdStmt::Raw("x();".into());
        track.baseline.push(original.clone());
        track.nodes = vec![fixture::node(
            1,
            NodeKind::Preserved {
                statement: original.clone(),
            },
        )];
        assert!(compile("mario", "attack11", &graph(track.clone())).is_ok());
        track.nodes[0].kind = NodeKind::Preserved {
            statement: AcmdStmt::Raw("y();".into()),
        };
        assert!(compile("mario", "attack11", &graph(track)).is_err());
    }

    #[test]
    fn reviewed_hash_cast_tokens_are_scoped_to_the_setter_value_field() {
        for descriptor in super::super::native_work::source_work_module_set_descriptors()
            .iter()
            .filter(|descriptor| descriptor.kind.is_int64())
        {
            for value in descriptor.values {
                let token = serde_json::Value::String((*value).into());
                assert!(validate_json(&token, "command.WorkModuleSet.value").is_ok());
                assert!(validate_json(&token, "command.WorkModuleSet.slot").is_err());
                assert!(validate_json(&token, "command.Attack.angle").is_err());
                assert!(validate_source_text(value).is_err());
                let command = ExcuteStmt::WorkModuleSet(crate::data::WorkModuleSetCall {
                    kind: descriptor.kind,
                    receiver: "agent.module_accessor".into(),
                    value: (*value).into(),
                    slot: descriptor.slot.into(),
                });
                assert!(validate_command(Some(descriptor.fighter), &command).is_ok());
                assert!(validate_command(Some("mario"), &command).is_err());
                let injected_value = format!("{value} + injected()");
                assert!(validate_json(
                    &serde_json::Value::String(injected_value),
                    "command.WorkModuleSet.value"
                )
                .is_err());
            }
        }
    }

    #[test]
    fn source_tokens_reject_code_injection_and_allow_bounded_constants() {
        for bad in [
            "FOO); system(\"x\"); (",
            "FIGHTER_MARIO_INSTANCE_WORK_ID_FLAG_SPECIAL_S_HOP",
            "1.0; panic!()",
            "\"x\"); agent.clear_lua_stack(); (\"",
            "std::process::exit(0)",
        ] {
            let track =
                fixture::track(Track::Game, vec![fixture::execute(1, vec![work_flag(bad)])]);
            assert!(
                compile("mario", "attack11", &graph(track)).is_err(),
                "accepted {bad:?}"
            );
        }
        let track = fixture::track(
            Track::Game,
            vec![fixture::execute(
                1,
                vec![ExcuteStmt::HitStatus {
                    target: crate::data::HurtTarget::Whole,
                    status: "*HIT_STATUS_NORMAL".into(),
                }],
            )],
        );
        assert!(compile("mario", "attack11", &graph(track)).is_ok());
        assert!(validate_command(
            Some("mario"),
            &work_flag("*FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO")
        )
        .is_ok());
        assert!(validate_source_text("Hash40::new(\"top\")").is_ok());
        assert!(validate_source_text("Hash40::new_raw(1)").is_ok());
        assert!(validate_source_text("Hash40::new(1)").is_err());
        assert!(validate_source_text("Hash40::new_raw(\"top\")").is_err());
    }

    #[test]
    fn emitted_names_and_macro_selectors_are_closed() {
        let bad_name = ExcuteStmt::Sound(crate::data::SoundCall {
            func: "PLAY_SE".into(),
            sounds: vec!["x\"); panic!(); (\"x".into()],
            tail: None,
        });
        assert!(validate_command(Some("mario"), &bad_name).is_err());
        let bad_macro = ExcuteStmt::Sound(crate::data::SoundCall {
            func: "PLAY_SE); panic!(); macros::PLAY_SE".into(),
            sounds: vec!["se_common_swing_01".into()],
            tail: None,
        });
        assert!(validate_command(Some("mario"), &bad_macro).is_err());
    }

    #[test]
    fn slope_forms_are_closed_bounded_and_kept_out_of_live_rules() {
        use crate::data::{SlopeCall, SlopeStatus, MAX_SLOPE_INTERPOLATION_FRAMES};

        let direct = ExcuteStmt::Slope(SlopeCall::Set {
            status: SlopeStatus::Lr,
        });
        let interpolated = ExcuteStmt::Slope(SlopeCall::Interpolate {
            status: SlopeStatus::Top,
            frames: 15,
            trailing_flag: Some(true),
        });
        assert!(validate_command(Some("mario"), &direct).is_ok());
        assert!(validate_command(Some("mario"), &interpolated).is_ok());
        assert!(!bare_command_matches_track(Track::Game, &direct));
        assert!(!bare_command_matches_track(Track::Expression, &direct));
        assert!(validate_command(
            Some("mario"),
            &ExcuteStmt::Slope(SlopeCall::Interpolate {
                status: SlopeStatus::None,
                frames: MAX_SLOPE_INTERPOLATION_FRAMES + 1,
                trailing_flag: None,
            })
        )
        .is_err());

        let authored = fixture::track(Track::Game, vec![fixture::execute(1, vec![direct])]);
        let authored_graph = graph(authored);
        assert!(
            super::super::live_support::source_only_tracks(&authored_graph)
                .any(|track| track == Track::Game),
            "a native slope command must withhold the game track from flat live rules"
        );

        let source = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![interpolated])],
        };
        for track in [Track::Game, Track::Expression] {
            let imported = import_for_fighter("mario", track, &source);
            assert!(
                imported.baseline.is_empty(),
                "{track:?} slope was not editable"
            );
            let graph = graph(imported);
            let compiled = compile("mario", "attack11", &graph).unwrap();
            assert_eq!(compiled.len(), 1);
            assert!(matches!(
                compiled[0].1.stmts.as_slice(),
                [AcmdStmt::Excute(commands)] if matches!(commands.as_slice(), [ExcuteStmt::Slope(_)])
            ));
        }
    }

    #[test]
    fn item_visibility_is_editable_only_in_game_and_expression_execute_blocks() {
        let command = ExcuteStmt::ItemVisibility { visible: false };
        assert!(validate_command(Some("mario"), &command).is_ok());
        assert!(!bare_command_matches_track(Track::Game, &command));
        assert!(!bare_command_matches_track(Track::Expression, &command));
        let source = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![command])],
        };
        for track in [Track::Game, Track::Expression] {
            let imported = import_for_fighter("mario", track, &source);
            assert!(
                imported.baseline.is_empty(),
                "{track:?} call was not editable"
            );
            let authored = graph(imported);
            let compiled = compile("mario", "attack11", &authored).unwrap();
            assert_eq!(compiled.len(), 1);
            assert!(matches!(
                compiled[0].1.stmts.as_slice(),
                [AcmdStmt::Excute(commands)]
                    if matches!(commands.as_slice(), [ExcuteStmt::ItemVisibility { visible: false }])
            ));
            assert!(
                super::super::live_support::source_only_tracks(&authored)
                    .any(|source_only| source_only == track),
                "native ItemModule source write must gate {track:?} flat live rules"
            );
        }
    }

    fn fighter_roundtrip(fighter: &str, track: Track, source: &AcmdScript) -> MoveTrack {
        let imported = import_for_fighter(fighter, track, source);
        let stored = serde_json::to_value(graph(imported.clone())).unwrap();
        let restored: MoveGraph = serde_json::from_value(stored).unwrap();
        let compiled = compile(fighter, "attack11", &restored).unwrap();
        assert_eq!(compiled[0].0, track);
        assert_json_eq!(source, &compiled[0].1);
        let reparsed = match track {
            Track::Game => crate::acmd::parse_acmd_script(&crate::acmd::preview_game_fn(
                &compiled[0].1,
                "attack11",
            )),
            Track::Expression => crate::acmd::parse_expression_script(
                &crate::acmd::preview_expression_fn(&compiled[0].1, "attack11"),
            ),
            _ => unreachable!(),
        };
        assert_json_eq!(source, reparsed);
        imported
    }

    #[test]
    fn root_native_commands_reject_wrong_tracks_guards_and_nested_authoring() {
        for (owner, command, tracks, bare) in [
            (
                "robot",
                ExcuteStmt::ShootItemBullet,
                &[Track::Game][..],
                false,
            ),
            (
                "buddy",
                ExcuteStmt::StartInfoFlashEye,
                &[Track::Game, Track::Expression][..],
                false,
            ),
            (
                "gaogaen",
                ExcuteStmt::CameraZoomOut,
                &[Track::Game][..],
                false,
            ),
            ("gaogaen", ExcuteStmt::StartCutIn, &[Track::Game][..], false),
            (
                "bayonetta",
                ExcuteStmt::CaptureCutCommon,
                &[Track::Game][..],
                true,
            ),
        ] {
            assert!(validate_command(Some(owner), &command).is_ok());
            for track in [Track::Game, Track::Expression, Track::Sound, Track::Effect] {
                assert_eq!(
                    command_matches_track(track, &command),
                    tracks.contains(&track) && !bare
                );
                assert_eq!(
                    bare_command_matches_track(track, &command),
                    tracks.contains(&track) && bare
                );
            }
            for &track in tracks {
                let statement = if bare {
                    AcmdStmt::Bare(Box::new(command.clone()))
                } else {
                    AcmdStmt::Excute(vec![
                        ExcuteStmt::Raw("Opaque::before();".into()),
                        command.clone(),
                        ExcuteStmt::Raw("Opaque::after();".into()),
                    ])
                };
                let source = AcmdScript {
                    stmts: vec![AcmdStmt::Frame(4.0), statement],
                };
                let imported = fighter_roundtrip(owner, track, &source);
                assert!(matches!(
                    imported.nodes[1].kind,
                    NodeKind::MixedExecute { .. } | NodeKind::BareCommand { .. }
                ));
                let authored = graph(imported);
                assert_eq!(
                    super::super::live_support::source_only_tracks(&authored).collect::<Vec<_>>(),
                    vec![track]
                );
                let mut wrong_track = authored.clone();
                wrong_track.tracks[0].track = Track::Sound;
                assert!(compile(owner, "attack11", &wrong_track).is_err());
                let mut wrong_guard = authored.clone();
                wrong_guard.tracks[0].nodes[1].kind = if bare {
                    NodeKind::Execute {
                        commands: vec![command.clone()],
                    }
                } else {
                    NodeKind::BareCommand {
                        command: Box::new(command.clone()),
                    }
                };
                assert!(compile(owner, "attack11", &wrong_guard).is_err());
                for kind in [
                    NodeKind::Repeat {
                        motion_loop: crate::data::MotionLoop::No,
                        count: 2,
                        body: authored.tracks[0].nodes.clone(),
                    },
                    NodeKind::Branch {
                        condition: MoveCondition::Grounded,
                        then_nodes: authored.tracks[0].nodes.clone(),
                        else_nodes: vec![],
                    },
                ] {
                    let mut nested = authored.clone();
                    nested.tracks[0].nodes = vec![fixture::node(900_100, kind)];
                    assert!(compile(owner, "attack11", &nested).is_err());
                }
            }
        }
    }

    #[test]
    fn typed_chart_event_source_roundtrips_as_a_game_only_export_command() {
        use crate::gimmicks::behavior::ConstValue;
        use crate::gimmicks::event_payload::Argument;
        let command = ExcuteStmt::GimmickChartEventValues {
            chart_id: "right".into(),
            name: "pulse".into(),
            arguments: vec![
                Argument {
                    field: "amount".into(),
                    value: ConstValue::Int(3),
                },
                Argument {
                    field: "gain".into(),
                    value: ConstValue::Float(0.5),
                },
                Argument {
                    field: "active".into(),
                    value: ConstValue::Flag(true),
                },
            ],
        };
        for track in [Track::Game, Track::Expression, Track::Sound, Track::Effect] {
            assert_eq!(command_matches_track(track, &command), track == Track::Game);
            assert!(!bare_command_matches_track(track, &command));
        }
        let source = AcmdScript {
            stmts: vec![AcmdStmt::Frame(2.0), AcmdStmt::Excute(vec![command])],
        };
        let text = crate::acmd::preview_game_fn(&source, "attack11");
        let parsed = crate::acmd::parse_acmd_script(&text);
        assert_json_eq!(&parsed, &source);
        let authored = graph(import_for_fighter("mario", Track::Game, &parsed));
        assert!(authored.tracks[0].baseline.is_empty());
        assert_json_eq!(
            &compile("mario", "attack11", &authored).unwrap()[0].1,
            &source
        );
        assert_eq!(
            super::super::live_support::source_only_tracks(&authored).collect::<Vec<_>>(),
            vec![Track::Game]
        );
    }

    #[test]
    fn model_default_visibility_roundtrips_and_rejects_foreign_or_forged_authoring() {
        let command = ExcuteStmt::VisibilitySetDefaultInt64 {
            group: "panel".into(),
        };
        assert!(validate_command(Some("gamewatch"), &command).is_ok());
        for (owner, track, editable) in [
            ("gamewatch", Track::Game, true),
            ("gamewatch", Track::Expression, true),
            ("pitb", Track::Game, false),
            ("unknown", Track::Game, false),
        ] {
            let source = AcmdScript {
                stmts: vec![AcmdStmt::Excute(vec![command.clone()])],
            };
            let imported = fighter_roundtrip(owner, track, &source);
            assert_eq!(
                matches!(imported.nodes[0].kind, NodeKind::Execute { .. }),
                editable
            );
            assert_eq!(
                matches!(imported.nodes[0].kind, NodeKind::Preserved { .. }),
                !editable
            );
        }
        for kind in [
            NodeKind::BareCommand {
                command: Box::new(command.clone()),
            },
            NodeKind::Repeat {
                motion_loop: crate::data::MotionLoop::No,
                count: 2,
                body: vec![fixture::execute(2, vec![command.clone()])],
            },
            NodeKind::Branch {
                condition: MoveCondition::Grounded,
                then_nodes: vec![fixture::execute(2, vec![command.clone()])],
                else_nodes: vec![],
            },
        ] {
            assert!(compile(
                "gamewatch",
                "attack11",
                &graph(fixture::track(Track::Game, vec![fixture::node(1, kind)]))
            )
            .is_err());
        }
        for (owner, track, group) in [
            ("pitb", Track::Game, "panel"),
            ("unknown", Track::Game, "panel"),
            ("gamewatch", Track::Sound, "panel"),
            ("gamewatch", Track::Effect, "panel"),
            ("gamewatch", Track::Game, "unmeasured_group"),
        ] {
            let command = ExcuteStmt::VisibilitySetDefaultInt64 {
                group: group.into(),
            };
            assert!(compile(
                owner,
                "attack11",
                &graph(fixture::track(
                    track,
                    vec![fixture::execute(1, vec![command])]
                ))
            )
            .is_err());
        }
        let legacy: ExcuteStmt = serde_json::from_value(serde_json::json!({
            "VisibilitySetInt64": {"group": "sword", "status": "sword_l"}
        }))
        .unwrap();
        assert!(
            matches!(legacy, ExcuteStmt::VisibilitySetInt64 { group, status }
            if group == "sword" && status == "sword_l")
        );
    }

    #[test]
    fn closed_native_commands_preserve_payloads_and_enforce_authoring_boundaries() {
        for (owner, kind, expression, foreign, root_only) in [
            ("cloud", 38, true, Some("mario"), true),
            ("mario", 96, true, None, true),
            ("koopajr", 98, true, Some("mario"), true),
            ("samus", 99, true, Some("mario"), true),
            ("mario", 72, false, None, false),
            ("mario", 73, false, None, false),
            ("robot", 93, false, Some("mario"), true),
            ("eflame", 94, false, Some("mario"), true),
            ("mario", 81, false, None, false),
            ("mario", 82, false, None, false),
            ("mario", 83, false, None, false),
            ("mario", 84, false, None, false),
            ("mario", 85, false, None, false),
            ("mario", 86, false, None, false),
        ] {
            let command = super::super::view::context_command(kind, owner);
            assert!(validate_command(Some(owner), &command).is_ok());
            for track in [Track::Game, Track::Expression, Track::Sound, Track::Effect] {
                let allowed = track == Track::Game || (expression && track == Track::Expression);
                assert_eq!(
                    command_matches_track(track, &command),
                    allowed,
                    "palette {kind}"
                );
                assert!(!bare_command_matches_track(track, &command));
                if !allowed {
                    continue;
                }
                let source = AcmdScript {
                    stmts: vec![AcmdStmt::Excute(vec![
                        ExcuteStmt::Raw("Opaque::before();".into()),
                        command.clone(),
                        ExcuteStmt::Raw("Opaque::after();".into()),
                    ])],
                };
                let imported = fighter_roundtrip(owner, track, &source);
                assert!(matches!(
                    imported.nodes[0].kind,
                    NodeKind::MixedExecute { .. }
                ));
                let authored = graph(imported);
                assert_eq!(
                    super::super::live_support::source_only_tracks(&authored).collect::<Vec<_>>(),
                    vec![track]
                );
                if let Some(foreign) = foreign {
                    assert!(validate_command(Some(foreign), &command).is_err());
                    assert!(matches!(
                        fighter_roundtrip(foreign, track, &source).nodes[0].kind,
                        NodeKind::Preserved { .. }
                    ));
                }
                let mut wrong_track = authored.clone();
                wrong_track.tracks[0].track = Track::Sound;
                assert!(compile(owner, "attack11", &wrong_track).is_err());
                assert!(matches!(
                    fighter_roundtrip(owner, Track::Game, &source).nodes[0].kind,
                    NodeKind::MixedExecute { .. }
                ));
                let wrong_track = import_for_fighter(owner, Track::Sound, &source);
                assert!(matches!(
                    wrong_track.nodes[0].kind,
                    NodeKind::Preserved { .. }
                ));
                assert_json_eq!(
                    &compile(owner, "attack11", &graph(wrong_track)).unwrap()[0].1,
                    &source
                );
                if root_only {
                    for kind in [
                        NodeKind::Repeat {
                            motion_loop: crate::data::MotionLoop::No,
                            count: 2,
                            body: authored.tracks[0].nodes.clone(),
                        },
                        NodeKind::Branch {
                            condition: MoveCondition::Grounded,
                            then_nodes: authored.tracks[0].nodes.clone(),
                            else_nodes: vec![],
                        },
                    ] {
                        let nested =
                            graph(fixture::track(track, vec![fixture::node(999_000, kind)]));
                        assert!(compile(owner, "attack11", &nested).is_err());
                    }
                }
            }
        }
        use crate::data::{FinalStartCameraCall, FinishCameraCall, SlowWholeCall};
        for (owner, command) in [
            (
                "mario",
                ExcuteStmt::SlowWhole(SlowWholeCall::Set {
                    multiplier: 7,
                    frames: 0,
                }),
            ),
            (
                "robot",
                ExcuteStmt::CheckFinishCamera(FinishCameraCall {
                    arg1: "999".into(),
                    arg2: "999".into(),
                }),
            ),
            (
                "eflame",
                ExcuteStmt::CheckFinalStartCamera(FinalStartCameraCall {
                    args: ["0", "7", "20", "0", "0", "6.4"].map(str::to_owned),
                }),
            ),
        ] {
            assert!(validate_command(Some(owner), &command).is_err());
        }
        for value in [
            serde_json::json!({"arg1": "15", "arg2": "5", "extra": "unknown"}),
            serde_json::json!({"arg1": 15, "arg2": "5"}),
        ] {
            assert!(serde_json::from_value::<FinishCameraCall>(value).is_err());
        }
        assert!(
            serde_json::from_value::<crate::data::AttackModuleDamageShakeScale>(serde_json::json!(
                "integer_zero"
            ))
            .is_err()
        );
        assert!(
            serde_json::from_value::<crate::data::AttackModuleNoUniqEffectAllPair>(
                serde_json::json!("false_false")
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<crate::data::AttackModuleTargetCategoryId>(serde_json::json!(
                "three"
            ))
            .is_err()
        );
    }

    #[test]
    fn article_state_expression_roundtrips_all_pairs_and_retains_root_only_scope() {
        for preset in crate::data::article_status_catalog::PRESETS {
            let source = AcmdScript {
                stmts: vec![AcmdStmt::Excute(vec![
                    ExcuteStmt::ArticleLifecycle(crate::data::ArticleLifecycleCall::ChangeStatus {
                        symbol: preset.article.into(),
                        status: preset.status.into(),
                        if_present: preset.if_present,
                    }),
                    ExcuteStmt::Raw("unknown_neighbor();".into()),
                ])],
            };
            let authored = graph(import_for_fighter(
                preset.fighter,
                Track::Expression,
                &source,
            ));
            assert!(matches!(
                authored.tracks[0].nodes[0].kind,
                NodeKind::MixedExecute { .. }
            ));
            let compiled = compile(preset.fighter, "attack11", &authored).unwrap();
            let emitted = crate::acmd::preview_expression_fn(&compiled[0].1, "attack11");
            let parsed = crate::acmd::parse_expression_script(&emitted);
            assert_json_eq!(&parsed, &source);
            let mut invalid = authored.clone();
            let root = invalid.tracks[0].nodes.remove(0);
            invalid.tracks[0].nodes.push(fixture::node(
                999,
                NodeKind::Repeat {
                    motion_loop: crate::data::MotionLoop::No,
                    count: 2,
                    body: vec![root],
                },
            ));
            assert!(compile(preset.fighter, "attack11", &invalid).is_err());
            for body in [
                format!(
                    "if runtime_condition() {{\n{}\n}}",
                    crate::acmd::emit_stmts(&source.stmts, "").join("\n")
                ),
                format!(
                    "#[cfg(any())]\n{}",
                    crate::acmd::emit_stmts(&source.stmts, "").join("\n")
                ),
            ] {
                let text = format!("unsafe extern \"C\" fn expression_test(agent: &mut L2CAgentBase) {{\n{body}\n}}\n");
                let opaque = import_for_fighter(
                    preset.fighter,
                    Track::Expression,
                    &crate::acmd::parse_expression_script(&text),
                );
                assert!(crate::move_nodes::test_support::untyped_condition(
                    &opaque.nodes[0].kind
                ));
                assert_eq!(
                    crate::move_nodes::coverage::track_summary(&opaque).editable_command_count,
                    0
                );
            }
        }
    }

    #[test]
    fn fixed_jostle_area_import_edit_persistence_and_source_only_roundtrip() {
        use crate::data::{FixedJostleAreaCall, FixedJostleAreaCall::EnableFix};

        // The two accepted statements are independent pinned-corpus call shapes. The malformed
        // neighbor must remain immutable source while the measured calls are authored.
        let source = crate::acmd::parse_acmd_script(
            r#"unsafe extern "C" fn game_fixed_jostle_area(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, 5.0, 6.5);
        FighterAreaModuleImpl::enable_fix_jostle_area_xy(agent.module_accessor, 2.0, 3.0, 3.0, 5.0);
        FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, 1.0);
    }
}
"#,
        );
        let valid_only = crate::acmd::parse_acmd_script(
            r#"unsafe extern "C" fn game_fixed_jostle_area_only(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, 5.0, 6.5);
        FighterAreaModuleImpl::enable_fix_jostle_area_xy(agent.module_accessor, 2.0, 3.0, 3.0, 5.0);
    }
}
"#,
        );
        let valid_only_graph = graph(import_for_fighter("bayonetta", Track::Game, &valid_only));
        assert!(matches!(
            valid_only_graph.tracks[0].nodes.as_slice(),
            [MoveNode { kind: NodeKind::Execute { commands }, .. }]
                if commands.len() == 2
        ));
        assert!(
            super::super::live_support::source_only_tracks(&valid_only_graph)
                .any(|track| track == Track::Game)
        );

        let imported = import_for_fighter("bayonetta", Track::Game, &source);
        assert!(
            !imported.baseline.is_empty(),
            "malformed neighbor stays baselined"
        );
        let mut authored = graph(imported);
        assert!(super::super::live_support::source_only_tracks(&authored)
            .any(|track| track == Track::Game));
        let [MoveNode {
            kind: NodeKind::MixedExecute { commands },
            ..
        }] = authored.tracks[0].nodes.as_mut_slice()
        else {
            panic!("valid fixed-area calls should be editable beside preserved source")
        };
        assert!(matches!(
            commands.as_slice(),
            [
                MoveCommand::Editable(ExcuteStmt::FixedJostleArea(
                    FixedJostleAreaCall::EnableFix { arg2, arg3 }
                )),
                MoveCommand::Editable(ExcuteStmt::FixedJostleArea(
                    FixedJostleAreaCall::EnableFixXy { arg2: x1, arg3: y1, arg4: x2, arg5: y2 }
                )),
                MoveCommand::Preserved(ExcuteStmt::Raw(_))
            ] if *arg2 == 5.0 && *arg3 == 6.5
                && *x1 == 2.0 && *y1 == 3.0 && *x2 == 3.0 && *y2 == 5.0
        ));

        if let MoveCommand::Editable(ExcuteStmt::FixedJostleArea(EnableFix { arg2, arg3 })) =
            &mut commands[0]
        {
            *arg2 = -2.0;
            *arg3 = 7.25;
        }
        if let MoveCommand::Editable(ExcuteStmt::FixedJostleArea(
            FixedJostleAreaCall::EnableFixXy {
                arg2,
                arg3,
                arg4,
                arg5,
            },
        )) = &mut commands[1]
        {
            *arg2 = 1.0;
            *arg3 = -3.5;
            *arg4 = 4.0;
            *arg5 = 8.0;
        }

        let saved = serde_json::to_value(&authored).unwrap();
        let restored: MoveGraph = serde_json::from_value(saved.clone()).unwrap();
        let compiled = compile("bayonetta", "fixed_jostle_area", &restored).unwrap();
        let emitted = crate::acmd::preview_game_fn(&compiled[0].1, "fixed_jostle_area");
        syn::parse_file(&emitted).expect("authored fixed-area source must parse as Rust");
        let reparsed = crate::acmd::parse_acmd_script(&emitted);
        assert_json_eq!(
            &compiled[0].1,
            &reparsed,
            "edited typed calls and immutable malformed neighbor must survive source roundtrip"
        );
        assert!(emitted.contains(
            "FighterAreaModuleImpl::enable_fix_jostle_area(agent.module_accessor, -2.0, 7.25);"
        ));
        assert!(emitted.contains(
            "FighterAreaModuleImpl::enable_fix_jostle_area_xy(agent.module_accessor, 1.0, -3.5, 4.0, 8.0);"
        ));
        assert_eq!(serde_json::to_value(&authored).unwrap(), saved);

        let before_rejection = serde_json::to_value(&authored).unwrap();
        let [MoveNode {
            kind: NodeKind::MixedExecute { commands },
            ..
        }] = authored.tracks[0].nodes.as_mut_slice()
        else {
            unreachable!("same imported mixed block")
        };
        let MoveCommand::Editable(ExcuteStmt::FixedJostleArea(EnableFix { arg2, .. })) =
            &mut commands[0]
        else {
            unreachable!("first command is the editable fixed-area call")
        };
        *arg2 = f32::INFINITY;
        let invalid_graph = serde_json::to_value(&authored).unwrap();
        assert!(compile("bayonetta", "fixed_jostle_area", &authored).is_err());
        assert_eq!(
            serde_json::to_value(&authored).unwrap(),
            invalid_graph,
            "rejected compilation must not mutate the source graph"
        );
        assert_ne!(invalid_graph, before_rejection);
    }

    #[test]
    fn attack_module_all_setters_are_closed_game_execute_commands() {
        use crate::data::AttackModuleSetAllKind;
        let no_dead = ExcuteStmt::AttackModuleSetAll {
            kind: AttackModuleSetAllKind::NoDeadAll,
            arg2: true,
            arg3: true,
        };
        let catch_only = ExcuteStmt::AttackModuleSetAll {
            kind: AttackModuleSetAllKind::CatchOnlyAll,
            arg2: true,
            arg3: false,
        };
        assert!(validate_command(Some("kirby"), &no_dead).is_ok());
        assert!(validate_command(Some("kirby"), &catch_only).is_ok());
        assert!(command_matches_track(Track::Game, &no_dead));
        assert!(command_matches_track(Track::Game, &catch_only));
        assert!(!command_matches_track(Track::Expression, &no_dead));
        assert!(!bare_command_matches_track(Track::Game, &no_dead));
        assert!(validate_command(
            Some("kirby"),
            &ExcuteStmt::AttackModuleSetAll {
                kind: AttackModuleSetAllKind::CatchOnlyAll,
                arg2: true,
                arg3: true,
            }
        )
        .is_err());
        assert!(validate_command(
            Some("kirby"),
            &ExcuteStmt::AttackModuleSetAll {
                kind: AttackModuleSetAllKind::NoDeadAll,
                arg2: false,
                arg3: false,
            }
        )
        .is_err());
    }

    #[test]
    fn slow_opponent_is_source_only_owned_and_preserves_unsupported_source() {
        use crate::data::{AcmdScript, AcmdStmt, ExcuteStmt, SlowOpponentCall};

        let command = ExcuteStmt::SlowOpponent(SlowOpponentCall {
            slow_mul: 13.0,
            slow_time: 60.0,
        });
        assert!(validate_command(Some("kirby"), &command).is_ok());
        assert!(command_matches_track(Track::Game, &command));
        assert!(!command_matches_track(Track::Sound, &command));
        assert!(!command_matches_track(Track::Expression, &command));
        assert!(!command_matches_track(Track::Effect, &command));
        assert!(!bare_command_matches_track(Track::Game, &command));

        let source = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![command.clone()])],
        };
        let imported = import_for_fighter("kirby", Track::Game, &source);
        let authored = graph(imported.clone());
        assert!(super::super::live_support::source_only_tracks(&authored)
            .any(|track| track == Track::Game));
        assert!(matches!(
            imported.nodes.as_slice(),
            [MoveNode { kind: NodeKind::Execute { commands }, .. }]
                if matches!(commands.as_slice(), [ExcuteStmt::SlowOpponent(_)])
        ));
        let compiled = compile("kirby", "attack11", &authored).unwrap();
        assert_json_eq!(&source, &compiled[0].1);
        let emitted = crate::acmd::preview_game_fn(&compiled[0].1, "attack11");
        let expected_call = format!(
            "macros::SLOW_OPPONENT(agent, {}, {});",
            crate::acmd::num(13.0),
            crate::acmd::num(60.0)
        );
        assert!(
            emitted.contains(&expected_call),
            "canonical emitter output: {emitted}"
        );
        assert_json_eq!(&source, crate::acmd::parse_acmd_script(&emitted));

        assert_json_eq!(
            &command,
            serde_json::from_value::<ExcuteStmt>(serde_json::to_value(&command).unwrap()).unwrap()
        );

        for invalid in [
            SlowOpponentCall {
                slow_mul: f32::NAN,
                slow_time: 60.0,
            },
            SlowOpponentCall {
                slow_mul: 1.0,
                slow_time: 60.0,
            },
            SlowOpponentCall {
                slow_mul: 13.0,
                slow_time: 131.0,
            },
        ] {
            let bad = ExcuteStmt::SlowOpponent(invalid);
            assert!(validate_command(Some("kirby"), &bad).is_err());
            let authored_bad = fixture::track(Track::Game, vec![fixture::execute(1, vec![bad])]);
            assert!(compile(
                "kirby",
                "attack11",
                &MoveGraph {
                    tracks: vec![authored_bad],
                    ..Default::default()
                }
            )
            .is_err());
        }

        let unknown_owner = import_for_fighter("not_a_fighter", Track::Game, &source);
        assert!(matches!(
            unknown_owner.nodes.as_slice(),
            [MoveNode {
                kind: NodeKind::Preserved { .. },
                ..
            }]
        ));
        assert!(
            super::super::live_support::source_only_tracks(&graph(imported))
                .any(|track| track == Track::Game)
        );

        let malformed_source = r#"unsafe extern "C" fn game_slow_opponent(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::SLOW_OPPONENT(agent, 13, 60);
        let broken = ;
    }
}
"#;
        assert!(syn::parse_file(malformed_source).is_err());
        let malformed = crate::acmd::parse_acmd_script(malformed_source);
        assert!(
            !malformed.stmts.is_empty(),
            "malformed function must not disappear into empty IR"
        );
        let malformed_graph = import_for_fighter("kirby", Track::Game, &malformed);
        assert!(!malformed_graph.nodes.is_empty());
        assert!(malformed_graph
            .nodes
            .iter()
            .all(|node| matches!(node.kind, NodeKind::Preserved { .. })));
        let malformed_graph = graph(malformed_graph);
        let graph_before = serde_json::to_value(&malformed_graph).unwrap();
        let errors = compile("kirby", "game_slow_opponent", &malformed_graph)
            .expect_err("malformed complete function must fail closed");
        assert!(errors.iter().all(|error| {
            error
                .reason
                .starts_with("generated ACMD failed Rust parsing:")
        }));
        assert_eq!(
            serde_json::to_value(&malformed_graph).unwrap(),
            graph_before
        );
        let malformed_emitted = crate::acmd::preview_game_fn(&malformed, "game_slow_opponent");
        assert!(malformed_emitted.contains("macros::SLOW_OPPONENT(agent, 13, 60);"));
        assert!(malformed_emitted.contains("let broken = ;"));
    }

    #[test]
    fn nana_camera_guard_is_source_only_and_preserves_neighboring_commands() {
        for move_name in ["final2", "finalair2"] {
            let source = crate::acmd::parse_acmd_script(&format!(
                r#"unsafe extern "C" fn game_{move_name}(agent: &mut L2CAgentBase) {{
    if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA) {{
        frame(agent.lua_state_agent, 4.0);
        if macros::is_excute(agent) {{
            macros::FT_SET_FINAL_FEAR_FACE(agent, 60);
            Native::preserved_neighbor(agent);
        }}
    }}
}}
"#
            ));
            let imported = import_for_fighter("nana", Track::Game, &source);
            let [MoveNode {
                kind:
                    NodeKind::Branch {
                        condition:
                            MoveCondition::WorkFlag {
                                symbol,
                                expected: false,
                            },
                        then_nodes,
                        else_nodes,
                    },
                ..
            }] = imported.nodes.as_slice()
            else {
                panic!("exact Nana source guard must stay structural");
            };
            assert_eq!(
                symbol,
                "FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA"
            );
            assert!(else_nodes.is_empty());
            assert!(
                matches!(&then_nodes[1].kind, NodeKind::MixedExecute { commands }
                if matches!(commands.as_slice(), [
                    MoveCommand::Editable(crate::data::ExcuteStmt::FinalFearFace(call)),
                    MoveCommand::Preserved(crate::data::ExcuteStmt::Raw(_))
                ] if call.argument == "60"))
            );
            let authored = graph(imported);
            assert!(super::super::live_support::source_only_tracks(&authored)
                .any(|track| track == Track::Game));
            let compiled = compile("nana", move_name, &authored).unwrap();
            assert_json_eq!(&source, &compiled[0].1);
            let emitted = crate::acmd::preview_game_fn(&compiled[0].1, move_name);
            assert_json_eq!(&source, crate::acmd::parse_acmd_script(&emitted));
            assert!(crate::gimmicks::registry::fighter_kind_symbol("nana").is_none());
        }
    }

    #[test]
    fn final_fear_face_keeps_the_exact_read_only_guard_else_and_rejoin_order() {
        use crate::data::{AcmdStmt, ExcuteStmt};

        let text = r#"unsafe extern "C" fn game_finalstart(agent: &mut L2CAgentBase) {
    if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA) {
        frame(agent.lua_state_agent, 4.0);
        if macros::is_excute(agent) {
            macros::FT_SET_FINAL_FEAR_FACE(agent, 60);
        }
    }
    else {
        frame(agent.lua_state_agent, 9.0);
    }
    frame(agent.lua_state_agent, 20.0);
}
"#;
        let source = crate::acmd::parse_acmd_script(text);
        assert!(matches!(
            source.stmts.as_slice(),
            [AcmdStmt::RawBlock { header, .. }, AcmdStmt::RawBlock { header: otherwise, .. }, AcmdStmt::Frame(20.0)]
                if header == "if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA) {"
                    && otherwise == "else {"
        ));

        let imported = import_for_fighter("robot", Track::Game, &source);
        let [MoveNode {
            kind:
                NodeKind::Branch {
                    condition:
                        MoveCondition::WorkFlag {
                            symbol,
                            expected: false,
                        },
                    then_nodes,
                    else_nodes,
                },
            ..
        }, MoveNode {
            kind: NodeKind::Frame { frame: 20.0 },
            ..
        }] = imported.nodes.as_slice()
        else {
            panic!(
                "the flag condition and following frame must remain structural siblings: {:?}",
                imported.nodes
            );
        };
        assert_eq!(
            symbol,
            "FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA"
        );
        assert!(matches!(
            then_nodes.first().map(|node| &node.kind),
            Some(NodeKind::Frame { frame: 4.0 })
        ));
        assert!(matches!(
            then_nodes.get(1).map(|node| &node.kind),
            Some(NodeKind::Execute { commands }) if matches!(commands.as_slice(), [ExcuteStmt::FinalFearFace(call)] if call.argument == "60")
        ));
        assert!(matches!(
            else_nodes.as_slice(),
            [MoveNode {
                kind: NodeKind::Frame { frame: 9.0 },
                ..
            }]
        ));
        assert!(validate_command(
            Some("robot"),
            &ExcuteStmt::FinalFearFace(crate::data::FinalFearFaceCall {
                argument: "60".into()
            })
        )
        .is_ok());
        assert!(validate_command(
            Some("mario"),
            &ExcuteStmt::FinalFearFace(crate::data::FinalFearFaceCall {
                argument: "7".into()
            })
        )
        .is_err());
        assert!(
            validate_command(
                Some("robot"),
                &ExcuteStmt::WorkFlag(crate::data::WorkFlagCall {
                    action: crate::data::WorkFlagAction::On,
                    flag: "*FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA".into(),
                })
            )
            .is_err(),
            "the read-only condition flag must not become an authored write"
        );
        assert!(!command_matches_track(
            Track::Sound,
            &ExcuteStmt::FinalFearFace(crate::data::FinalFearFaceCall {
                argument: "60".into()
            })
        ));
        assert!(!command_matches_track(
            Track::Expression,
            &ExcuteStmt::FinalFearFace(crate::data::FinalFearFaceCall {
                argument: "60".into()
            })
        ));

        let authored = graph(imported);
        assert!(super::super::live_support::source_only_tracks(&authored)
            .any(|track| track == Track::Game));
        let compiled = compile("robot", "finalstart", &authored).unwrap();
        let emitted = crate::acmd::preview_game_fn(&compiled[0].1, "finalstart");
        assert!(emitted.contains("if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA) {"));
        assert!(emitted.contains("macros::FT_SET_FINAL_FEAR_FACE(agent, 60);"));
        assert!(emitted.contains("    else {\n        frame(agent.lua_state_agent, 9.0);"));
        assert!(emitted.contains("    frame(agent.lua_state_agent, 20.0);"));
        assert_json_eq!(
            &source,
            crate::acmd::parse_acmd_script(&emitted),
            "branch, execute, else and rejoin order must survive graph compilation"
        );
        let json =
            serde_json::to_value(ExcuteStmt::FinalFearFace(crate::data::FinalFearFaceCall {
                argument: "60".into(),
            }))
            .unwrap();
        let restored: ExcuteStmt = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), json);
    }

    #[test]
    fn final_fear_face_unknown_guards_and_invalid_contexts_remain_source() {
        let unknown_guard = r#"unsafe extern "C" fn game_finalstart(agent: &mut L2CAgentBase) {
    if(methodlib::L2CValue::operator==(lib::L2CValueconst&)const(false, false)){
        if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA) {
            if macros::is_excute(agent) {
                macros::FT_SET_FINAL_FEAR_FACE(agent, 60);
            }
        }
    }
}
"#;
        let source = crate::acmd::parse_acmd_script(unknown_guard);
        let imported = import_for_fighter("robot", Track::Game, &source);
        assert!(matches!(
            imported.nodes.as_slice(),
            [MoveNode {
                kind: NodeKind::Preserved { .. },
                ..
            }]
        ));
        let emitted = crate::acmd::preview_game_fn(&source, "finalstart");
        assert_json_eq!(
            &source,
            crate::acmd::parse_acmd_script(&emitted),
            "the decompiled guard must not be rewritten or its child promoted"
        );

        let attributed_guard = crate::acmd::parse_acmd_script(
            r#"unsafe extern "C" fn game_finalstart(agent: &mut L2CAgentBase) {
    #[cfg(any())]
    if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA) {
        if macros::is_excute(agent) {
            macros::FT_SET_FINAL_FEAR_FACE(agent, 60);
        }
    }
}
"#,
        );
        let attributed_import = import_for_fighter("robot", Track::Game, &attributed_guard);
        let attributed_compiled =
            compile("robot", "finalstart", &graph(attributed_import)).unwrap();
        assert_json_eq!(&attributed_guard, &attributed_compiled[0].1, "an attributed exact guard and its child must remain source-stable through graph compilation");

        for malformed in [
            "macros::FT_SET_FINAL_FEAR_FACE(boma, 60);",
            "macros::FT_SET_FINAL_FEAR_FACE(agent);",
            "macros::FT_SET_FINAL_FEAR_FACE(agent, 60, 1);",
            "#[cfg(any())]\n        macros::FT_SET_FINAL_FEAR_FACE(agent, 60);",
            "macros::FT_SET_FINAL_FEAR_FACE(agent, 999);",
        ] {
            let text = format!(
                r#"unsafe extern "C" fn game_finalstart(agent: &mut L2CAgentBase) {{
    if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA) {{
        if macros::is_excute(agent) {{
            {malformed}
        }}
    }}
}}
"#
            );
            let source = crate::acmd::parse_acmd_script(&text);
            fn count_typed(stmts: &[crate::data::AcmdStmt]) -> usize {
                stmts
                    .iter()
                    .map(|stmt| match stmt {
                        crate::data::AcmdStmt::Excute(commands) => commands
                            .iter()
                            .filter(|command| {
                                matches!(command, crate::data::ExcuteStmt::FinalFearFace(_))
                            })
                            .count(),
                        crate::data::AcmdStmt::RawBlock { body, .. } => count_typed(body),
                        _ => 0,
                    })
                    .sum()
            }
            assert_eq!(
                count_typed(&source.stmts),
                0,
                "invalid guarded statement became typed: {malformed}"
            );
            let imported = import_for_fighter("robot", Track::Game, &source);
            let compiled = compile("robot", "finalstart", &graph(imported)).unwrap();
            assert_json_eq!(&source, &compiled[0].1);
        }

        let wrong_owner_source = crate::acmd::parse_acmd_script(
            r#"unsafe extern "C" fn game_finalstart(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::FT_SET_FINAL_FEAR_FACE(agent, 7);
    }
}
"#,
        );
        let wrong_owner = import_for_fighter("mario", Track::Game, &wrong_owner_source);
        assert!(matches!(
            wrong_owner.nodes.as_slice(),
            [MoveNode {
                kind: NodeKind::Preserved { .. },
                ..
            }]
        ));
        let wrong_owner_compiled = compile("mario", "finalstart", &graph(wrong_owner)).unwrap();
        assert_json_eq!(
            &wrong_owner_source,
            &wrong_owner_compiled[0].1,
            "a globally known literal for the wrong owner remains unchanged source"
        );

        let bare = crate::acmd::parse_acmd_script(
            r#"unsafe extern "C" fn game_finalstart(agent: &mut L2CAgentBase) {
    macros::FT_SET_FINAL_FEAR_FACE(agent, 60);
}
"#,
        );
        let bare = import_for_fighter("robot", Track::Game, &bare);
        assert!(matches!(
            bare.nodes.as_slice(),
            [MoveNode {
                kind: NodeKind::Preserved { .. },
                ..
            }]
        ));
        let sound = crate::acmd::parse_sound_script(
            r#"unsafe extern "C" fn sound_finalstart(agent: &mut L2CAgentBase) {
    if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_FLAG_DISABLE_FINAL_START_CAMERA) {
        if macros::is_excute(agent) {
            macros::FT_SET_FINAL_FEAR_FACE(agent, 60);
        }
    }
}
"#,
        );
        let sound = import_for_fighter("robot", Track::Sound, &sound);
        assert!(
            matches!(
                sound.nodes.as_slice(),
                [MoveNode { kind: NodeKind::Branch { then_nodes, .. }, .. }]
                    if matches!(then_nodes.as_slice(), [MoveNode { kind: NodeKind::Preserved { .. }, .. }])
            ),
            "the guard may remain visible, but the Game-only child must be preserved on Sound"
        );
    }

    #[test]
    fn optional_hit_feedback_validation_rejects_unbounded_authored_values() {
        use crate::data::{
            ExcuteStmt, Hash40Literal, OptionalHitFeedbackCall, OptionalHitFeedbackKind,
        };

        let invalid_calls = [
            OptionalHitFeedbackCall {
                kind: OptionalHitFeedbackKind::Effect,
                arg2: -1,
                hash: Hash40Literal::Name("known_shape".into()),
            },
            OptionalHitFeedbackCall {
                kind: OptionalHitFeedbackKind::Sound,
                arg2: 5,
                hash: Hash40Literal::Raw(u64::MAX),
            },
            OptionalHitFeedbackCall {
                kind: OptionalHitFeedbackKind::Effect,
                arg2: 0,
                hash: Hash40Literal::Name(String::new()),
            },
            OptionalHitFeedbackCall {
                kind: OptionalHitFeedbackKind::Effect,
                arg2: 0,
                hash: Hash40Literal::Name("x".repeat(256)),
            },
        ];
        for call in invalid_calls {
            let command = ExcuteStmt::OptionalHitFeedback(call);
            assert!(validate_command(Some("mario"), &command).is_err());
            // A deserialized/authored typed variant has no source baseline to rely on, so the
            // compiler must still validate every bounded value before emitting native source.
            let authored = MoveGraph {
                tracks: vec![fixture::track(
                    Track::Game,
                    vec![fixture::execute(1, vec![command])],
                )],
                ..Default::default()
            };
            let errors = compile("mario", "attack11", &authored)
                .expect_err("invalid typed optional feedback must fail closed");
            assert!(errors
                .iter()
                .any(|error| error.reason.contains("optional hit feedback arg2")));
        }

        let valid = ExcuteStmt::OptionalHitFeedback(OptionalHitFeedbackCall {
            kind: OptionalHitFeedbackKind::Sound,
            arg2: 0,
            hash: Hash40Literal::Name("custom_character_value".into()),
        });
        assert!(validate_command(Some("mario"), &valid).is_ok());
        assert!(validate_command(Some("not_a_fighter"), &valid).is_err());
        assert!(command_matches_track(Track::Game, &valid));
        assert!(!command_matches_track(Track::Sound, &valid));
        assert!(!command_matches_track(Track::Expression, &valid));
        assert!(!command_matches_track(Track::Effect, &valid));
        assert!(!bare_command_matches_track(Track::Game, &valid));
        for track in [Track::Sound, Track::Expression, Track::Effect] {
            let wrong_track = MoveGraph {
                tracks: vec![fixture::track(
                    track,
                    vec![fixture::execute(1, vec![valid.clone()])],
                )],
                ..Default::default()
            };
            assert!(compile("mario", "attack11", &wrong_track).is_err());
        }
    }

    #[test]
    fn indexed_attack_and_grab_rebound_commands_are_closed_source_only_forms() {
        use crate::data::{AttackModuleSetIndexedKind, ExcuteStmt};

        let camera = ExcuteStmt::AttackModuleSetIndexed {
            kind: AttackModuleSetIndexedKind::NoFinishCamera,
            id: 3,
            arg3: true,
            arg4: false,
        };
        let reaction = ExcuteStmt::AttackModuleSetIndexed {
            kind: AttackModuleSetIndexedKind::ForceReaction,
            id: 0,
            arg3: true,
            arg4: false,
        };
        let rebound = ExcuteStmt::GrabRebound { enabled: true };
        assert!(validate_command(Some("mario"), &camera).is_ok());
        assert!(validate_command(Some("kirby"), &reaction).is_ok());
        assert!(validate_command(Some("koopa"), &rebound).is_ok());
        for command in [&camera, &reaction, &rebound] {
            assert!(command_matches_track(Track::Game, command));
            assert!(!command_matches_track(Track::Expression, command));
            assert!(!bare_command_matches_track(Track::Game, command));
        }
        for invalid in [
            ExcuteStmt::AttackModuleSetIndexed {
                kind: AttackModuleSetIndexedKind::NoFinishCamera,
                id: -1,
                arg3: true,
                arg4: false,
            },
            ExcuteStmt::AttackModuleSetIndexed {
                kind: AttackModuleSetIndexedKind::ForceReaction,
                id: 4,
                arg3: true,
                arg4: false,
            },
            ExcuteStmt::AttackModuleSetIndexed {
                kind: AttackModuleSetIndexedKind::ForceReaction,
                id: 1,
                arg3: false,
                arg4: false,
            },
            ExcuteStmt::AttackModuleSetIndexed {
                kind: AttackModuleSetIndexedKind::ForceReaction,
                id: 1,
                arg3: true,
                arg4: true,
            },
        ] {
            assert!(validate_command(Some("kirby"), &invalid).is_err());
        }
    }

    #[test]
    fn attack_height_and_final_finish_commands_are_closed_game_only_source_forms() {
        use crate::data::{AttackHeightAllKind, ExcuteStmt};

        let height = ExcuteStmt::AttackModuleSetHeightAll {
            height: AttackHeightAllKind::High,
            arg3: false,
        };
        let final_finish = ExcuteStmt::AttackModuleSetFinalFinishCutIn {
            id: 3,
            arg3: true,
            arg4: false,
            arg5: -1.0,
            arg6: false,
        };
        for command in [&height, &final_finish] {
            assert!(validate_command(Some("kirby"), command).is_ok());
            assert!(command_matches_track(Track::Game, command));
            assert!(!command_matches_track(Track::Expression, command));
            assert!(!bare_command_matches_track(Track::Game, command));
        }

        for invalid in [
            ExcuteStmt::AttackModuleSetHeightAll {
                height: AttackHeightAllKind::Low,
                arg3: true,
            },
            ExcuteStmt::AttackModuleSetFinalFinishCutIn {
                id: -1,
                arg3: true,
                arg4: false,
                arg5: -1.0,
                arg6: false,
            },
            ExcuteStmt::AttackModuleSetFinalFinishCutIn {
                id: 0,
                arg3: true,
                arg4: false,
                arg5: f32::NAN,
                arg6: false,
            },
            ExcuteStmt::AttackModuleSetFinalFinishCutIn {
                id: 0,
                arg3: true,
                arg4: false,
                arg5: f32::INFINITY,
                arg6: false,
            },
        ] {
            assert!(validate_command(Some("kirby"), &invalid).is_err());
        }
    }

    #[test]
    fn revised_reaction_frames_edit_inside_mixed_source_without_changing_neighbors() {
        let source = r#"unsafe extern "C" fn game_revised(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 8.0);
    if macros::is_excute(agent) {
        opaque_native_neighbor();
        AttackModule::set_add_reaction_frame_revised(agent.module_accessor, 0, 11.0, false);
        AttackModule::set_add_reaction_frame(agent.module_accessor, 4, 3.0, false);
    }
}"#;
        let parsed = crate::acmd::parse_acmd_script(source);
        let mut imported = import_for_fighter("trail", Track::Game, &parsed);
        let NodeKind::MixedExecute { commands } = &mut imported.nodes[1].kind else {
            panic!("mixed source import")
        };
        assert!(matches!(commands[0], MoveCommand::Preserved(_)));
        let MoveCommand::Editable(ExcuteStmt::AttackModuleAddReactionFrameRevised { id, frames }) =
            &mut commands[1]
        else {
            panic!("revised editable")
        };
        *id = 3;
        *frames = -4.5;
        let authored = graph(imported);
        let persisted: MoveGraph =
            serde_json::from_str(&serde_json::to_string(&authored).unwrap()).unwrap();
        let compiled = compile("trail", "revised", &persisted).unwrap();
        let preview = crate::acmd::preview_game_fn(&compiled[0].1, "revised");
        assert!(preview.contains("opaque_native_neighbor();"));
        assert!(preview.contains(
            "AttackModule::set_add_reaction_frame_revised(agent.module_accessor, 3, -4.5, false);"
        ));
        assert!(preview.contains(
            "AttackModule::set_add_reaction_frame(agent.module_accessor, 4, 3.0, false);"
        ));
        assert_json_eq!(&compiled[0].1, crate::acmd::parse_acmd_script(&preview));
        for invalid in [
            ExcuteStmt::AttackModuleAddReactionFrameRevised { id: 9, frames: 1.0 },
            ExcuteStmt::AttackModuleAddReactionFrameRevised {
                id: 0,
                frames: f32::NAN,
            },
            ExcuteStmt::AttackModuleAddReactionFrameRevised {
                id: 0,
                frames: f32::INFINITY,
            },
        ] {
            assert!(validate_command(Some("trail"), &invalid).is_err());
        }
    }

    #[test]
    fn standard_throw_application_bundle_imports_compiles_and_reparses_in_scope() {
        let source = r#"unsafe extern "C" fn game_throw(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        let target = WorkModule::get_int64(agent.module_accessor, *FIGHTER_STATUS_THROW_WORK_INT_TARGET_OBJECT);
        let target_group = WorkModule::get_int64(agent.module_accessor, *FIGHTER_STATUS_THROW_WORK_INT_TARGET_HIT_GROUP);
        let target_no = WorkModule::get_int64(agent.module_accessor, *FIGHTER_STATUS_THROW_WORK_INT_TARGET_HIT_NO);
        macros::ATK_HIT_ABS(agent, *FIGHTER_ATTACK_ABSOLUTE_KIND_THROW, Hash40::new("throw"), target, target_group, target_no);
        AttackModule::clear_all(agent.module_accessor);
    }
}
"#;
        let parsed = crate::acmd::parse_acmd_script(source);
        let imported = import_for_fighter("mario", Track::Game, &parsed);
        assert!(matches!(
            imported.nodes.get(1).map(|node| &node.kind),
            Some(NodeKind::Execute { commands })
                if matches!(commands.as_slice(), [
                    ExcuteStmt::AttackAbsApplyThrow,
                    ExcuteStmt::ClearAll,
                ])
        ));

        let compiled = compile("mario", "throw_f", &graph(imported)).unwrap();
        let preview = crate::acmd::preview_game_fn(&compiled[0].1, "throw_f");
        assert!(preview.contains(
            "let target = WorkModule::get_int64(agent.module_accessor, *FIGHTER_STATUS_THROW_WORK_INT_TARGET_OBJECT);"
        ));
        assert!(preview.contains(
            "macros::ATK_HIT_ABS(agent, *FIGHTER_ATTACK_ABSOLUTE_KIND_THROW, Hash40::new(\"throw\"), target, target_group, target_no);"
        ));
        assert!(preview.contains("AttackModule::clear_all(agent.module_accessor);"));
        assert_json_eq!(
            parsed,
            crate::acmd::parse_acmd_script(&preview),
            "the bundle must recreate its locals in the original guarded scope"
        );

        let legacy = source.replace(
            "FIGHTER_ATTACK_ABSOLUTE_KIND_THROW, Hash40::new(\"throw\")",
            "FIGHTER_ATTACK_ABSOLUTE_KIND_THROW_MEWTWO, Hash40::new(\"throw\")",
        );
        let legacy_parsed = crate::acmd::parse_acmd_script(&legacy);
        let legacy_imported = import_for_fighter("mario", Track::Game, &legacy_parsed);
        assert!(matches!(
            legacy_imported.nodes.get(1).map(|node| &node.kind),
            Some(NodeKind::MixedExecute { commands }) if matches!(commands.as_slice(), [
                MoveCommand::Preserved(ExcuteStmt::Raw(_)),
                MoveCommand::Preserved(ExcuteStmt::Raw(_)),
                MoveCommand::Preserved(ExcuteStmt::Raw(_)),
                MoveCommand::Preserved(ExcuteStmt::Raw(_)),
                MoveCommand::Editable(ExcuteStmt::ClearAll),
            ])
        ));
        let legacy_compiled = compile("mario", "throw_f", &graph(legacy_imported)).unwrap();
        let legacy_preview = crate::acmd::preview_game_fn(&legacy_compiled[0].1, "throw_f");
        assert_json_eq!(
            legacy_parsed,
            crate::acmd::parse_acmd_script(&legacy_preview),
            "a modified kind must remain opaque through timeline compilation"
        );

        let authored_wrong_track = graph(fixture::track(
            Track::Expression,
            vec![fixture::execute(1, vec![ExcuteStmt::AttackAbsApplyThrow])],
        ));
        assert!(
            compile("mario", "throw_f", &authored_wrong_track).is_err(),
            "the fixed bundle is valid only on the Game track"
        );
        let authored_bare = graph(fixture::track(
            Track::Game,
            vec![fixture::node(
                1,
                NodeKind::BareCommand {
                    command: Box::new(ExcuteStmt::AttackAbsApplyThrow),
                },
            )],
        ));
        assert!(
            compile("mario", "throw_f", &authored_bare).is_err(),
            "target reads and application cannot be authored outside execute scope"
        );
    }

    #[test]
    fn reaction_frame_authored_values_are_bounded_and_wrong_track_stays_preserved() {
        let valid = ExcuteStmt::AttackModuleAddReactionFrame {
            id: 8,
            frames: -10.0,
        };
        assert!(validate_command(Some("mario"), &valid).is_ok());
        assert!(command_matches_track(Track::Game, &valid));
        assert!(!command_matches_track(Track::Expression, &valid));
        assert!(!command_matches_track(Track::Sound, &valid));
        assert!(!bare_command_matches_track(Track::Game, &valid));

        for invalid in [
            ExcuteStmt::AttackModuleAddReactionFrame { id: 9, frames: 1.0 },
            ExcuteStmt::AttackModuleAddReactionFrame {
                id: 0,
                frames: f32::NAN,
            },
            ExcuteStmt::AttackModuleAddReactionFrame {
                id: 0,
                frames: f32::INFINITY,
            },
        ] {
            assert!(validate_command(Some("mario"), &invalid).is_err());
            let imported = import_for_fighter(
                "mario",
                Track::Game,
                &AcmdScript {
                    stmts: vec![AcmdStmt::Excute(vec![invalid])],
                },
            );
            assert!(matches!(
                imported.nodes.first().map(|node| &node.kind),
                Some(NodeKind::Preserved { .. })
            ));
        }

        let expression = crate::acmd::parse_expression_script(
            r#"unsafe extern "C" fn expression_wrong_track(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        AttackModule::set_add_reaction_frame(agent.module_accessor, 0, 2.0, false);
    }
}
"#,
        );
        let imported = import_for_fighter("mario", Track::Expression, &expression);
        assert!(matches!(
            imported.nodes.first().map(|node| &node.kind),
            Some(NodeKind::Preserved { .. })
        ));
    }

    #[test]
    fn reviewed_attack_module_points_and_reference_tuples_are_closed_game_forms() {
        use crate::data::{AttackReferenceJointKind, ExcuteStmt};

        let init = ExcuteStmt::AttackModuleInitAttackPos { id: 0 };
        let references = [
            AttackReferenceJointKind::TopYZX,
            AttackReferenceJointKind::TopYNoneNone,
            AttackReferenceJointKind::ToerYMinusXZ,
        ]
        .map(|kind| ExcuteStmt::AttackModuleSetAttackReferenceJointId { kind });
        let smoke = ExcuteStmt::AttackModuleSetNoDamageFlySmokeAll {
            arg2: true,
            arg3: false,
        };
        for command in [&init, &smoke] {
            assert!(validate_command(Some("kirby"), command).is_ok());
            assert!(command_matches_track(Track::Game, command));
            assert!(!command_matches_track(Track::Expression, command));
            assert!(!bare_command_matches_track(Track::Game, command));
        }
        for command in &references {
            assert!(validate_command(Some("kirby"), command).is_ok());
            assert!(command_matches_track(Track::Game, command));
            assert!(command_matches_track(Track::Expression, command));
            assert!(!command_matches_track(Track::Sound, command));
            assert!(!command_matches_track(Track::Effect, command));
            assert!(!bare_command_matches_track(Track::Expression, command));
        }
        for kind in AttackReferenceJointKind::EXPRESSION_CHOICES {
            let command = ExcuteStmt::AttackModuleSetAttackReferenceJointId { kind };
            assert_eq!(
                command_matches_track(Track::Game, &command),
                AttackReferenceJointKind::GAME_CHOICES.contains(&kind)
            );
            assert!(command_matches_track(Track::Expression, &command));
            assert!(!command_matches_track(Track::Sound, &command));
            assert!(!command_matches_track(Track::Effect, &command));
        }
        assert_eq!(
            AttackReferenceJointKind::TopYZX.native_parts(),
            (
                "top",
                "ATTACK_DIRECTION_Y",
                "ATTACK_DIRECTION_Z",
                "ATTACK_DIRECTION_X"
            )
        );

        for invalid in [
            ExcuteStmt::AttackModuleInitAttackPos { id: 1 },
            ExcuteStmt::AttackModuleSetNoDamageFlySmokeAll {
                arg2: true,
                arg3: true,
            },
            ExcuteStmt::AttackModuleSetNoDamageFlySmokeAll {
                arg2: false,
                arg3: false,
            },
        ] {
            assert!(validate_command(Some("kirby"), &invalid).is_err());
        }
    }

    #[test]
    fn expression_article_motion_catalog_keeps_owner_scope_copy_context_and_source_only_boundary() {
        use crate::data::{ArticleLifecycleCall, EXPRESSION_ARTICLE_MOTIONS};
        for &(symbol, motions) in EXPRESSION_ARTICLE_MOTIONS {
            let fighter = symbol
                .strip_prefix("FIGHTER_")
                .unwrap()
                .split_once("_GENERATE_ARTICLE_")
                .unwrap()
                .0
                .to_ascii_lowercase();
            registry::resolve(&fighter, symbol, IdentifierKind::Article).unwrap();
            for &motion in motions {
                let command = ExcuteStmt::ArticleLifecycle(ArticleLifecycleCall::ChangeMotion {
                    symbol: symbol.into(),
                    motion: motion.into(),
                });
                assert!(
                    validate_command_on_track(Some(&fighter), Track::Expression, &command).is_ok()
                );
                assert!(
                    validate_command_on_track(Some("mario"), Track::Expression, &command).is_err()
                );
                assert!(validate_command_on_track(None, Track::Expression, &command).is_err());
                assert!(
                    validate_command(Some(&fighter), &command).is_err(),
                    "original Game validator stays closed"
                );
                assert!(root_command_context_allowed_on_track(
                    Track::Expression,
                    0,
                    &command
                ));
                assert!(!root_command_context_allowed_on_track(
                    Track::Expression,
                    1,
                    &command
                ));
                let source = AcmdScript {
                    stmts: vec![AcmdStmt::Excute(vec![command.clone()])],
                };
                let imported = import_for_fighter(&fighter, Track::Expression, &source);
                assert!(imported.baseline.is_empty());
                let authored = graph(imported);
                assert_eq!(
                    super::super::live_support::source_only_tracks(&authored).collect::<Vec<_>>(),
                    vec![Track::Expression]
                );
                let serialized = serde_json::to_value(&authored).unwrap();
                let restored: MoveGraph = serde_json::from_value(serialized.clone()).unwrap();
                let compiled = compile(&fighter, "article_motion", &restored).unwrap();
                assert_json_eq!(&compiled[0].1, &source);
                assert!(compiled[0].1.to_expression_events().is_empty());
                for track in [Track::Game, Track::Sound, Track::Effect] {
                    let mut wrong = restored.clone();
                    wrong.tracks[0].track = track;
                    assert!(
                        compile(&fighter, "article_motion", &wrong).is_err(),
                        "{track:?} {symbol} {motion}"
                    );
                    assert!(!command_matches_track(track, &command));
                }
                let mut wrong_json = serialized;
                wrong_json["tracks"][0]["track"] = serde_json::json!("game");
                let wrong: MoveGraph = serde_json::from_value(wrong_json).unwrap();
                assert!(compile(&fighter, "article_motion", &wrong).is_err());
                assert!(compile("mario", "article_motion", &restored).is_err());
                let mut bare = restored.clone();
                bare.tracks[0].nodes[0].kind = NodeKind::BareCommand {
                    command: Box::new(command.clone()),
                };
                assert!(compile(&fighter, "article_motion", &bare).is_err());
                for kind in [
                    NodeKind::Repeat {
                        motion_loop: crate::data::MotionLoop::No,
                        count: 2,
                        body: restored.tracks[0].nodes.clone(),
                    },
                    NodeKind::Branch {
                        condition: MoveCondition::Grounded,
                        then_nodes: restored.tracks[0].nodes.clone(),
                        else_nodes: Vec::new(),
                    },
                ] {
                    let mut nested = restored.clone();
                    nested.tracks[0].nodes = vec![fixture::node(99, kind)];
                    assert!(compile(&fighter, "article_motion", &nested).is_err());
                }
                let mut copied = restored.clone();
                let mut copy = copied.tracks[0].nodes[0].clone();
                copy.id += 100;
                copied.tracks[0].nodes.push(copy);
                let expected = AcmdScript {
                    stmts: vec![
                        AcmdStmt::Excute(vec![command.clone()]),
                        AcmdStmt::Excute(vec![command.clone()]),
                    ],
                };
                assert_json_eq!(
                    &compile(&fighter, "article_motion", &copied).unwrap()[0].1,
                    expected
                );
                let mixed = AcmdScript {
                    stmts: vec![AcmdStmt::Excute(vec![
                        ExcuteStmt::Raw("unknown_before();".into()),
                        command.clone(),
                        ExcuteStmt::Raw("unknown_after();".into()),
                    ])],
                };
                let mixed_graph = graph(import_for_fighter(&fighter, Track::Expression, &mixed));
                assert!(
                    matches!(&mixed_graph.tracks[0].nodes[0].kind, NodeKind::MixedExecute { commands } if matches!(commands.as_slice(), [MoveCommand::Preserved(_), MoveCommand::Editable(_), MoveCommand::Preserved(_)]))
                );
                assert_json_eq!(
                    &compile(&fighter, "article_motion", &mixed_graph).unwrap()[0].1,
                    &mixed
                );
            }
        }
        let foreign = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![ExcuteStmt::ArticleLifecycle(
                ArticleLifecycleCall::ChangeMotion {
                    symbol: "FIGHTER_SAMUSD_GENERATE_ARTICLE_GUN".into(),
                    motion: "air_catch_pose".into(),
                },
            )])],
        };
        let foreign_graph = graph(import_for_fighter("samus", Track::Expression, &foreign));
        assert!(matches!(
            foreign_graph.tracks[0].nodes[0].kind,
            NodeKind::Preserved { .. }
        ));
        assert_json_eq!(
            &compile("samus", "air_catch_landing", &foreign_graph).unwrap()[0].1,
            foreign
        );
        let nested_source = crate::acmd::parse_expression_script("unsafe extern \"C\" fn expression_motion_nested(agent: &mut L2CAgentBase) {\n    for _ in 0..2 {\n        if macros::is_excute(agent) {\n            ArticleModule::change_motion(agent.module_accessor, *FIGHTER_METAKNIGHT_GENERATE_ARTICLE_MANTLE, Hash40::new(\"guard\"), false, -1.0);\n        }\n    }\n}\n");
        assert!(!serde_json::to_string(&nested_source)
            .unwrap()
            .contains("ChangeMotion"));
        let nested_graph = graph(import_for_fighter(
            "metaknight",
            Track::Expression,
            &nested_source,
        ));
        assert_json_eq!(
            &compile("metaknight", "guard", &nested_graph).unwrap()[0].1,
            nested_source
        );
        let legacy = AcmdScript {
            stmts: vec![AcmdStmt::Loop {
                motion_loop: crate::data::MotionLoop::No,
                count: 2,
                body: vec![AcmdStmt::Excute(vec![ExcuteStmt::ArticleLifecycle(
                    ArticleLifecycleCall::ChangeMotion {
                        symbol: "FIGHTER_KIRBY_GENERATE_ARTICLE_STONE".into(),
                        motion: "special_lw".into(),
                    },
                )])],
            }],
        };
        let legacy_graph = graph(import_for_fighter("kirby", Track::Game, &legacy));
        assert_json_eq!(
            &compile("kirby", "special_lw", &legacy_graph).unwrap()[0].1,
            legacy
        );
    }

    #[test]
    fn expression_reference_joint_import_serde_copy_and_live_boundary_stay_source_only() {
        use crate::data::{AttackReferenceJointKind, ExcuteStmt};

        let kind = *AttackReferenceJointKind::EXPRESSION_CHOICES
            .iter()
            .find(|kind| !AttackReferenceJointKind::GAME_CHOICES.contains(kind))
            .expect("Expression catalog contains Expression-only tuples");
        let command = ExcuteStmt::AttackModuleSetAttackReferenceJointId { kind };
        let (bone, axis3, axis4, axis5) = kind.native_parts();
        let valid_call = format!(
            "AttackModule::set_attack_reference_joint_id(agent.module_accessor, Hash40::new(\"{bone}\"), AttackDirectionAxis(*{axis3}), AttackDirectionAxis(*{axis4}), AttackDirectionAxis(*{axis5}));"
        );
        let source = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![command.clone()])],
        };
        assert!(source.to_expression_events().is_empty());

        let imported = import_for_fighter("falco", Track::Expression, &source);
        assert!(imported.baseline.is_empty());
        let authored = graph(imported);
        assert_eq!(
            super::super::live_support::source_only_tracks(&authored).collect::<Vec<_>>(),
            vec![Track::Expression]
        );
        let serialized = serde_json::to_value(&authored).unwrap();
        let restored: MoveGraph = serde_json::from_value(serialized.clone()).unwrap();
        let compiled = compile("falco", "attack11", &restored).unwrap();
        assert_eq!(compiled.len(), 1);
        assert_json_eq!(&compiled[0].1, &source);
        assert!(compiled[0].1.to_expression_events().is_empty());

        let mut copied = restored.clone();
        copied.tracks[0].nodes[0].id += 1000;
        let copied_kind = copied.tracks[0].nodes[0].kind.clone();
        let copied_id = copied.tracks[0].nodes[0].id + 1;
        copied.tracks[0]
            .nodes
            .push(fixture::node(copied_id, copied_kind));
        let copied_source = AcmdScript {
            stmts: vec![
                AcmdStmt::Excute(vec![command.clone()]),
                AcmdStmt::Excute(vec![command.clone()]),
            ],
        };
        assert_json_eq!(
            &compile("falco", "attack11", &copied).unwrap()[0].1,
            &copied_source
        );

        for invalid_track in [Track::Game, Track::Sound, Track::Effect] {
            let mut invalid = restored.clone();
            invalid.tracks[0].track = invalid_track;
            assert!(
                compile("falco", "attack11", &invalid).is_err(),
                "{invalid_track:?}"
            );
        }
        let mut wrong_track_json = serialized;
        wrong_track_json["tracks"][0]["track"] = serde_json::json!("game");
        let deserialized_wrong_track: MoveGraph = serde_json::from_value(wrong_track_json).unwrap();
        assert!(compile("falco", "attack11", &deserialized_wrong_track).is_err());

        let mut bare = restored.clone();
        bare.tracks[0].nodes[0].kind = crate::move_nodes::model::NodeKind::BareCommand {
            command: Box::new(command.clone()),
        };
        assert!(compile("falco", "attack11", &bare).is_err());

        for kind in AttackReferenceJointKind::EXPRESSION_CHOICES {
            let command = ExcuteStmt::AttackModuleSetAttackReferenceJointId { kind };
            assert!(root_command_context_allowed_on_track(
                Track::Expression,
                0,
                &command
            ));
            assert!(!root_command_context_allowed_on_track(
                Track::Expression,
                1,
                &command
            ));
            if AttackReferenceJointKind::GAME_CHOICES.contains(&kind) {
                assert!(root_command_context_allowed_on_track(
                    Track::Game,
                    1,
                    &command
                ));
                assert!(command_matches_track(Track::Game, &command));
            } else {
                assert!(!command_matches_track(Track::Game, &command));
            }
        }

        let nested_text = format!(
            "unsafe extern \"C\" fn expression_reference_nested(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        if true {{ {valid_call} }}\n    }}\n}}\n"
        );
        let nested_source = crate::acmd::parse_expression_script(&nested_text);
        assert!(!serde_json::to_string(&nested_source)
            .unwrap()
            .contains("AttackModuleSetAttackReferenceJointId"));
        let nested_imported = import_for_fighter("falco", Track::Expression, &nested_source);
        assert!(!nested_imported.baseline.is_empty());
        let nested_graph = graph(nested_imported);
        assert_json_eq!(
            &compile("falco", "attack11", &nested_graph).unwrap()[0].1,
            nested_source
        );

        let expression_child = restored.tracks[0].nodes.clone();
        for wrapper in [
            crate::move_nodes::model::NodeKind::Repeat {
                motion_loop: crate::data::MotionLoop::No,
                count: 2,
                body: expression_child.clone(),
            },
            crate::move_nodes::model::NodeKind::Branch {
                condition: crate::move_nodes::model::MoveCondition::Grounded,
                then_nodes: expression_child.clone(),
                else_nodes: Vec::new(),
            },
        ] {
            let mut nested_graph = restored.clone();
            nested_graph.tracks[0].nodes = vec![fixture::node(400_000, wrapper)];
            assert!(compile("falco", "attack11", &nested_graph).is_err());
        }

        let legacy_game_command = ExcuteStmt::AttackModuleSetAttackReferenceJointId {
            kind: AttackReferenceJointKind::TopYZX,
        };
        let legacy_game_nested = AcmdScript {
            stmts: vec![AcmdStmt::Loop {
                motion_loop: crate::data::MotionLoop::No,
                count: 2,
                body: vec![AcmdStmt::Excute(vec![legacy_game_command])],
            }],
        };
        let legacy_game_graph = graph(import_for_fighter(
            "falco",
            Track::Game,
            &legacy_game_nested,
        ));
        assert_json_eq!(
            &compile("falco", "attack11", &legacy_game_graph).unwrap()[0].1,
            legacy_game_nested
        );
    }

    #[test]
    fn article_commands_roundtrip_with_exact_owner_track_and_guard_boundaries() {
        use crate::data::{ArticleLifecycleCall as Article, ArticleTarget};
        let sword = "FIGHTER_GANON_GENERATE_ARTICLE_SWORD".to_owned();
        let banana = "FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA".to_owned();
        for (owner, call, tracks) in [
            (
                "ganon",
                Article::Generate {
                    symbol: sword.clone(),
                },
                &[Track::Game, Track::Expression][..],
            ),
            (
                "ganon",
                Article::RemoveAll { symbol: sword },
                &[Track::Game, Track::Expression][..],
            ),
            (
                "diddy",
                Article::SetVisibilityWhole {
                    symbol: "FIGHTER_DIDDY_GENERATE_ARTICLE_BARRELJETS".into(),
                    visible: false,
                },
                &[Track::Game, Track::Expression][..],
            ),
            (
                "diddy",
                Article::Shoot {
                    symbol: banana.clone(),
                    target: ArticleTarget::All,
                },
                &[Track::Game][..],
            ),
            (
                "diddy",
                Article::ShootExist {
                    symbol: banana,
                    target: ArticleTarget::Last,
                },
                &[Track::Game, Track::Expression][..],
            ),
            (
                "kirby",
                Article::ChangeMotion {
                    symbol: "FIGHTER_KIRBY_GENERATE_ARTICLE_STONE".into(),
                    motion: "special_lw".into(),
                },
                &[Track::Game][..],
            ),
        ] {
            let command = ExcuteStmt::ArticleLifecycle(call);
            assert!(validate_command(Some(owner), &command).is_ok());
            assert!(validate_command(None, &command).is_err());
            assert!(validate_command(Some("mario"), &command).is_err());
            for track in [Track::Game, Track::Expression, Track::Sound, Track::Effect] {
                assert_eq!(
                    command_matches_track(track, &command),
                    tracks.contains(&track)
                );
                assert!(!bare_command_matches_track(track, &command));
            }
            for &track in tracks {
                let source = AcmdScript {
                    stmts: vec![AcmdStmt::Excute(vec![command.clone()])],
                };
                let imported = fighter_roundtrip(owner, track, &source);
                assert!(imported.baseline.is_empty());
                assert!(matches!(imported.nodes[0].kind, NodeKind::Execute { .. }));
                let foreign = fighter_roundtrip("mario", track, &source);
                assert!(matches!(foreign.nodes[0].kind, NodeKind::Preserved { .. }));
            }
        }
        for call in [
            Article::Generate {
                symbol: "24".into(),
            },
            Article::RemoveAll {
                symbol: "FIGHTER_GANON_GENERATE_ARTICLE_MISSING".into(),
            },
            Article::ChangeMotion {
                symbol: "FIGHTER_KIRBY_GENERATE_ARTICLE_STONE".into(),
                motion: "special_hi".into(),
            },
        ] {
            let owner = if matches!(call, Article::ChangeMotion { .. }) {
                "kirby"
            } else {
                "ganon"
            };
            let authored = graph(fixture::track(
                Track::Game,
                vec![fixture::execute(
                    1,
                    vec![ExcuteStmt::ArticleLifecycle(call)],
                )],
            ));
            assert!(compile(owner, "attack11", &authored).is_err());
        }
        for invalid in [
            serde_json::json!({"SetVisibilityWhole": {"symbol": "FIGHTER_ROBOT_GENERATE_ARTICLE_GYRO", "visible": "true"}}),
            serde_json::json!({"Shoot": {"symbol": "FIGHTER_DIDDY_GENERATE_ARTICLE_ITEM_BANANA", "target": "First"}}),
        ] {
            assert!(serde_json::from_value::<Article>(invalid).is_err());
        }
    }

    #[test]
    fn gimmick_event_is_a_bounded_game_execute_receiver_only() {
        use crate::data::ExcuteStmt;

        let command = ExcuteStmt::GimmickEvent {
            name: "ready".into(),
        };
        assert!(validate_command(None, &command).is_ok());
        assert!(command_matches_track(Track::Game, &command));
        assert!(!command_matches_track(Track::Expression, &command));
        assert!(!command_matches_track(Track::Sound, &command));
        assert!(!bare_command_matches_track(Track::Game, &command));

        for name in ["", "   ", "bad\0name"] {
            assert!(
                validate_command(None, &ExcuteStmt::GimmickEvent { name: name.into() }).is_err()
            );
        }
        assert!(validate_command(
            None,
            &ExcuteStmt::GimmickEvent {
                name: "x".repeat(crate::gimmicks::live_model::MAX_NAME_BYTES + 1),
            }
        )
        .is_err());

        let track = fixture::track(
            Track::Game,
            vec![fixture::execute(1, vec![command.clone()])],
        );
        let output = compile("mario", "attack11", &graph(track)).unwrap();
        assert!(output[0].1.to_hitboxes().is_empty());
        let emitted = crate::acmd::preview_game_fn(&output[0].1, "attack11");
        assert!(emitted.contains(
            "crate::slight::gimmicks::dispatch_named_from_module(agent.module_accessor, \"ready\");"
        ));
        assert_json_eq!(
            &output[0].1,
            AcmdScript {
                stmts: vec![AcmdStmt::Excute(vec![command])],
            }
        );
    }

    #[test]
    fn work_module_set_int_uses_i32_bounds_and_measured_receivers() {
        let set_int = |value: &str, receiver: &str| {
            ExcuteStmt::WorkModuleSet(crate::data::WorkModuleSetCall {
                kind: crate::data::WorkModuleSetKind::Int,
                receiver: receiver.into(),
                value: value.into(),
                slot: "*FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_BAT_WITH_IN_STATE".into(),
            })
        };
        for value in [i32::MIN.to_string(), i32::MAX.to_string()] {
            assert!(
                validate_command(Some("bayonetta"), &set_int(&value, "agent.module_accessor"))
                    .is_ok()
            );
        }
        for value in ["-2147483649", "2147483648"] {
            assert!(
                validate_command(Some("bayonetta"), &set_int(value, "agent.module_accessor"))
                    .is_err()
            );
        }
        assert!(
            validate_command(Some("bayonetta"), &set_int("1", "other.module_accessor")).is_err()
        );
        let bare_slot = ExcuteStmt::WorkModuleSet(crate::data::WorkModuleSetCall {
            kind: crate::data::WorkModuleSetKind::Int,
            receiver: "agent.module_accessor".into(),
            value: "1".into(),
            slot: "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_BAT_WITH_IN_STATE".into(),
        });
        assert!(validate_command(Some("bayonetta"), &bare_slot).is_err());
    }

    #[test]
    fn item_grass_pull_rejects_wrong_pairs_receivers_types_and_placements() {
        use crate::data::WorkModuleSetCall;
        use crate::move_nodes::native_work as native;

        let call = |kind, receiver: &str, value: &str, slot: &str| {
            ExcuteStmt::WorkModuleSet(WorkModuleSetCall {
                kind,
                receiver: receiver.into(),
                value: value.into(),
                slot: slot.into(),
            })
        };
        let invalid = [
            call(
                crate::data::WorkModuleSetKind::Int,
                "agent.module_accessor",
                "*FIGHTER_ITEM_GRASS_PULL_STEP_OTHER",
                native::ITEM_GRASS_PULL_NEXT_STEP_SLOT,
            ),
            call(
                crate::data::WorkModuleSetKind::Int,
                "agent.module_accessor",
                "*FIGHTER_ITEM_GRASS_PULL_STEP_PICKUP + 1",
                native::ITEM_GRASS_PULL_NEXT_STEP_SLOT,
            ),
            call(
                crate::data::WorkModuleSetKind::Int,
                "agent.module_accessor",
                "4",
                native::ITEM_GRASS_PULL_NEXT_STEP_SLOT,
            ),
            call(
                crate::data::WorkModuleSetKind::Int,
                "agent.module_accessor",
                "5",
                native::THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX_SLOT,
            ),
            call(
                crate::data::WorkModuleSetKind::Int,
                "boma",
                native::ITEM_GRASS_PULL_STEP_VALUES[0],
                native::ITEM_GRASS_PULL_NEXT_STEP_SLOT,
            ),
            call(
                crate::data::WorkModuleSetKind::Float,
                "agent.module_accessor",
                "4.0",
                native::THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX_SLOT,
            ),
            call(
                crate::data::WorkModuleSetKind::Int64,
                "agent.module_accessor",
                "4",
                native::THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX_SLOT,
            ),
            call(
                crate::data::WorkModuleSetKind::Int,
                "agent.module_accessor",
                native::ITEM_GRASS_PULL_STEP_VALUES[0],
                native::ITEM_GRASS_PULL_NEXT_STEP_SLOT.trim_start_matches('*'),
            ),
            call(
                crate::data::WorkModuleSetKind::Int,
                "agent.module_accessor",
                "*FIGHTER_ITEM_GRASS_PULL_STEP_PICKUP",
                "*FIGHTER_STATUS_ITEM_GRASS_PULL_WORK_INT_UNKNOWN",
            ),
        ];
        for command in invalid {
            let source = AcmdScript {
                stmts: vec![AcmdStmt::Excute(vec![command])],
            };
            assert!(matches!(
                import_for_fighter("cloud", Track::Game, &source).nodes[0].kind,
                NodeKind::Preserved { .. }
            ));
        }
        let accepted = call(
            crate::data::WorkModuleSetKind::Int,
            "agent.module_accessor",
            native::ITEM_GRASS_PULL_STEP_VALUES[0],
            native::ITEM_GRASS_PULL_NEXT_STEP_SLOT,
        );
        assert!(matches!(
            import_for_fighter(
                "not_a_fighter",
                Track::Game,
                &AcmdScript {
                    stmts: vec![AcmdStmt::Excute(vec![accepted.clone()])],
                },
            )
            .nodes[0]
                .kind,
            NodeKind::Preserved { .. }
        ));

        for track in [Track::Sound, Track::Expression, Track::Effect] {
            let source = AcmdScript {
                stmts: vec![AcmdStmt::Excute(vec![accepted.clone()])],
            };
            assert!(matches!(
                import_for_fighter("cloud", track, &source).nodes[0].kind,
                NodeKind::Preserved { .. }
            ));
        }
        let bare = import_for_fighter(
            "cloud",
            Track::Game,
            &AcmdScript {
                stmts: vec![AcmdStmt::Bare(Box::new(accepted.clone()))],
            },
        );
        assert!(matches!(bare.nodes[0].kind, NodeKind::Preserved { .. }));

        for track in [Track::Sound, Track::Expression, Track::Effect] {
            let misplaced =
                fixture::track(track, vec![fixture::execute(1, vec![accepted.clone()])]);
            assert!(compile("cloud", "item_grass_pull", &graph(misplaced)).is_err());
        }
        let bare_authored = fixture::track(
            Track::Game,
            vec![fixture::node(
                1,
                NodeKind::BareCommand {
                    command: Box::new(accepted),
                },
            )],
        );
        assert!(compile("cloud", "item_grass_pull", &graph(bare_authored)).is_err());
    }

    #[test]
    fn work_module_inc_int_requires_a_fighter_owned_work_int_constant() {
        let inc_int = |slot: &str| {
            ExcuteStmt::WorkModuleIncInt(crate::data::WorkModuleIncIntCall { slot: slot.into() })
        };
        let owned_slot = "*FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_BAT_WITH_IN_STATE";
        assert!(validate_command(Some("bayonetta"), &inc_int(owned_slot)).is_ok());
        assert!(validate_command(Some("mario"), &inc_int(owned_slot)).is_err());
        assert!(validate_command(
            Some("bayonetta"),
            &inc_int("*FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_AIR_SPECIAL_S_U_TO_D")
        )
        .is_err());
        for invalid in [
            "42",
            "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_BAT_WITH_IN_STATE",
            "*FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_BAT_WITH_IN_STATE + 1",
        ] {
            assert!(
                validate_command(Some("bayonetta"), &inc_int(invalid)).is_err(),
                "accepted unsafe WorkModule::inc_int slot {invalid:?}"
            );
        }

        let source = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![inc_int(owned_slot)])],
        };
        let imported = import_for_fighter("bayonetta", Track::Game, &source);
        assert!(matches!(imported.nodes[0].kind, NodeKind::Execute { .. }));
        let compiled = compile("bayonetta", "attack11", &graph(imported)).unwrap();
        assert_json_eq!(&compiled[0].1, source);

        for (fighter, slot) in [
            ("mario", owned_slot),
            (
                "bayonetta",
                "*FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_AIR_SPECIAL_S_U_TO_D",
            ),
            ("bayonetta", "42"),
            (
                "bayonetta",
                "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_BAT_WITH_IN_STATE",
            ),
        ] {
            let imported = import_for_fighter(
                fighter,
                Track::Game,
                &AcmdScript {
                    stmts: vec![AcmdStmt::Excute(vec![inc_int(slot)])],
                },
            );
            assert!(
                matches!(imported.nodes[0].kind, NodeKind::Preserved { .. }),
                "unsafe WorkModule::inc_int slot {slot:?} became editable for {fighter}"
            );
        }

        let raw_syntax = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![ExcuteStmt::Raw(
                "WorkModule::inc_int(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_BAT_WITH_IN_STATE + 1);".into(),
            )])],
        };
        assert!(matches!(
            import_for_fighter("bayonetta", Track::Game, &raw_syntax).nodes[0].kind,
            NodeKind::Preserved { .. }
        ));
    }

    #[test]
    fn work_module_set_float_requires_a_finite_f32_literal() {
        let set_float = |value: &str| {
            ExcuteStmt::WorkModuleSet(crate::data::WorkModuleSetCall {
                kind: crate::data::WorkModuleSetKind::Float,
                receiver: "agent.module_accessor".into(),
                value: value.into(),
                slot: "*FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLOAT_SPECIAL_LW_SLOW_TIME".into(),
            })
        };
        for valid in ["1.0", "-1.0", "1e0f32", "1.0f32"] {
            assert!(
                validate_command(Some("bayonetta"), &set_float(valid)).is_ok(),
                "rejected valid f32 literal {valid:?}"
            );
        }
        for invalid in ["1", "1.0f64", "NaN", "1.0 + 0.0", "1e100"] {
            assert!(
                validate_command(Some("bayonetta"), &set_float(invalid)).is_err(),
                "accepted invalid f32 token {invalid:?}"
            );
        }
    }

    #[test]
    fn mixed_execute_edits_known_command_and_roundtrips_opaque_sibling() {
        let source = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![
                attack(),
                ExcuteStmt::Raw("custom_native_call(agent, 7);".into()),
                ExcuteStmt::ClearAll,
            ])],
        };
        let imported = import_for_fighter("mario", Track::Game, &source);
        let schema_value = serde_json::to_value(&imported).unwrap();
        let schema_roundtrip: MoveTrack = serde_json::from_value(schema_value.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(schema_roundtrip).unwrap(),
            schema_value
        );
        assert_eq!(imported.baseline.len(), 1);
        let NodeKind::MixedExecute { commands } = &imported.nodes[0].kind else {
            panic!("partly editable execute block was not imported as mixed");
        };
        assert!(matches!(
            commands[0],
            MoveCommand::Editable(ExcuteStmt::Attack(_))
        ));
        assert!(matches!(
            commands[1],
            MoveCommand::Preserved(ExcuteStmt::Raw(_))
        ));
        assert!(matches!(
            commands[2],
            MoveCommand::Editable(ExcuteStmt::ClearAll)
        ));

        let compiled = compile("mario", "attack11", &graph(imported.clone())).unwrap();
        assert_json_eq!(&compiled[0].1, &source);

        let mut edited = imported;
        let NodeKind::MixedExecute { commands } = &mut edited.nodes[0].kind else {
            unreachable!();
        };
        let MoveCommand::Editable(ExcuteStmt::Attack(call)) = &mut commands[0] else {
            unreachable!();
        };
        call.damage = 9.0;
        let compiled = compile("mario", "attack11", &graph(edited)).unwrap();
        let AcmdStmt::Excute(commands) = &compiled[0].1.stmts[0] else {
            panic!("mixed execute block moved outside its native execute guard");
        };
        let ExcuteStmt::Attack(edited_attack) = &commands[0] else {
            panic!("edited attack command changed families");
        };
        assert_eq!(edited_attack.damage, 9.0);
        assert_json_eq!(
            &commands[1],
            ExcuteStmt::Raw("custom_native_call(agent, 7);".into())
        );
        assert!(matches!(&commands[2], ExcuteStmt::ClearAll));
    }

    #[test]
    fn mixed_execute_rejects_opaque_mutation_duplicates_and_whole_wrapper_reuse() {
        let source = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![
                attack(),
                ExcuteStmt::Raw("opaque_a(agent);".into()),
                ExcuteStmt::Raw("opaque_b(agent);".into()),
            ])],
        };
        let imported = import_for_fighter("mario", Track::Game, &source);
        let mut mutated = imported.clone();
        let NodeKind::MixedExecute { commands } = &mut mutated.nodes[0].kind else {
            panic!("expected mixed import");
        };
        commands[1] = MoveCommand::Preserved(ExcuteStmt::Raw("injected();".into()));
        assert!(compile("mario", "attack11", &graph(mutated)).is_err());

        let mut raw_authored = imported.clone();
        let NodeKind::MixedExecute { commands } = &mut raw_authored.nodes[0].kind else {
            panic!("expected mixed import");
        };
        commands[1] = MoveCommand::Editable(ExcuteStmt::Raw("injected();".into()));
        assert!(compile("mario", "attack11", &graph(raw_authored)).is_err());

        let mut duplicated = imported.clone();
        let NodeKind::MixedExecute { commands } = &mut duplicated.nodes[0].kind else {
            panic!("expected mixed import");
        };
        commands[1] = commands[2].clone();
        assert!(compile("mario", "attack11", &graph(duplicated)).is_err());

        let mut double_claim = imported;
        let preserved = double_claim.baseline[0].clone();
        double_claim.nodes.push(fixture::node(
            100,
            NodeKind::Preserved {
                statement: preserved,
            },
        ));
        assert!(compile("mario", "attack11", &graph(double_claim)).is_err());
    }

    #[test]
    fn execute_import_uses_mixed_nodes_only_when_some_commands_are_editable() {
        let all_known = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![attack(), ExcuteStmt::ClearAll])],
        };
        assert!(matches!(
            import_for_fighter("mario", Track::Game, &all_known).nodes[0].kind,
            NodeKind::Execute { .. }
        ));
        let all_unknown = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![ExcuteStmt::Raw(
                "custom_a();".into(),
            )])],
        };
        assert!(matches!(
            import_for_fighter("mario", Track::Game, &all_unknown).nodes[0].kind,
            NodeKind::Preserved { .. }
        ));
    }

    #[test]
    fn dash_to_run_mixed_execute_keeps_neighbors_and_rejects_other_terms() {
        use crate::data::{
            WorkTransitionTermAction as Action, WorkTransitionTermCall,
            WorkTransitionTermReceiver as Receiver,
        };

        let source = crate::acmd::parse_acmd_script(
            r#"unsafe extern "C" fn game_dash(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        WorkModule::enable_transition_term(agent.module_accessor, *FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN);
        WorkModule::unable_transition_term(agent.module_accessor, *FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN);
        WorkModule::enable_transition_term(agent.module_accessor, 17);
        WorkModule::enable_transition_term(agent.module_accessor, *FIGHTER_DIDDY_STATUS_SPECIAL_S_TRANSITION_TERM_ID_GROUND);
        WorkModule::enable_transition_term(agent.module_accessor, *FUTURE_TRANSITION_TERM);
        WorkModule::enable_transition_term(agent.module_accessor, *FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN, false);
        WorkModule::enable_transition_term(boma, *FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN);
        custom_native_call(agent.module_accessor, 7);
        AttackModule::clear_all(agent.module_accessor);
    }
}
"#,
        );
        let imported = import_for_fighter("mario", Track::Game, &source);
        let NodeKind::MixedExecute { commands } = &imported.nodes[0].kind else {
            panic!("supported term with opaque siblings should become mixed Execute");
        };
        assert!(matches!(
            commands[0],
            MoveCommand::Editable(ExcuteStmt::WorkTransitionTerm(_))
        ));
        for command in &commands[1..=4] {
            assert!(matches!(
                command,
                MoveCommand::Preserved(ExcuteStmt::WorkTransitionTerm(_))
            ));
        }
        assert!(matches!(
            commands[5],
            MoveCommand::Preserved(ExcuteStmt::Raw(_))
        ));
        assert!(matches!(
            &commands[6],
            MoveCommand::Preserved(ExcuteStmt::WorkTransitionTerm(call))
                if call.receiver == Receiver::Boma
        ));
        assert!(matches!(
            commands[7],
            MoveCommand::Preserved(ExcuteStmt::Raw(_))
        ));
        assert!(matches!(
            commands[8],
            MoveCommand::Editable(ExcuteStmt::ClearAll)
        ));
        let compiled = compile("mario", "dash", &graph(imported.clone())).unwrap();
        assert_json_eq!(&compiled[0].1, &source);

        for invalid in [
            WorkTransitionTermCall {
                action: Action::Unable,
                receiver: Receiver::AgentAccessor,
                transition_term: "*FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN".into(),
            },
            WorkTransitionTermCall {
                action: Action::Enable,
                receiver: Receiver::AgentAccessor,
                transition_term: "17".into(),
            },
            WorkTransitionTermCall {
                action: Action::Enable,
                receiver: Receiver::AgentAccessor,
                transition_term: "*FIGHTER_DIDDY_STATUS_SPECIAL_S_TRANSITION_TERM_ID_GROUND".into(),
            },
            WorkTransitionTermCall {
                action: Action::Enable,
                receiver: Receiver::Boma,
                transition_term: "*FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN".into(),
            },
        ] {
            let mut edited = imported.clone();
            let NodeKind::MixedExecute { commands } = &mut edited.nodes[0].kind else {
                unreachable!();
            };
            commands[0] = MoveCommand::Editable(ExcuteStmt::WorkTransitionTerm(invalid));
            assert!(compile("mario", "dash", &graph(edited)).is_err());
        }

        let effect_import = import_for_fighter("mario", Track::Effect, &source);
        assert!(
            matches!(
                effect_import.nodes.as_slice(),
                [MoveNode {
                    kind: NodeKind::Preserved { .. },
                    ..
                }]
            ),
            "transition-term calls are Game-track only"
        );
    }

    #[test]
    fn hit_status_import_is_limited_to_the_four_reviewed_states() {
        let status = |value: &str| ExcuteStmt::HitStatus {
            target: crate::data::HurtTarget::Whole,
            status: value.into(),
        };
        for accepted in [
            "*HIT_STATUS_NORMAL",
            "*HIT_STATUS_INVINCIBLE",
            "*HIT_STATUS_XLU",
            "*HIT_STATUS_OFF",
            "HIT_STATUS_NORMAL",
            "HIT_STATUS_INVINCIBLE",
            "HIT_STATUS_XLU",
            "HIT_STATUS_OFF",
            "3",
        ] {
            let source = AcmdScript {
                stmts: vec![AcmdStmt::Excute(vec![status(accepted)])],
            };
            let imported = import_for_fighter("mario", Track::Game, &source);
            assert!(matches!(imported.nodes[0].kind, NodeKind::Execute { .. }));
            let compiled = compile("mario", "attack11", &graph(imported)).unwrap();
            assert_json_eq!(&compiled[0].1, source);
        }
        for rejected in [
            "*HIT_STATUS_TERM",
            "HIT_STATUS_NEW_STATE",
            "HIT_STATUS_MASK_NORMAL",
            "*HIT_STATUS_MASK_XLU",
            "StatusModule::status_kind(agent.module_accessor)",
            "HIT_STATUS_XLU); panic!(); (",
            "4",
            "-1",
        ] {
            let imported = import_for_fighter(
                "mario",
                Track::Game,
                &AcmdScript {
                    stmts: vec![AcmdStmt::Excute(vec![status(rejected)])],
                },
            );
            assert!(matches!(imported.nodes[0].kind, NodeKind::Preserved { .. }));
        }
    }

    #[test]
    fn parsed_hit_status_target_forms_import_compile_and_reparse_exactly() {
        let original = crate::acmd::parse_acmd_script(
            r#"unsafe extern "C" fn game_attack11(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::HIT_NODE(agent, Hash40::new("kneer"), *HIT_STATUS_XLU);
        macros::HIT_NO(agent, 3, *HIT_STATUS_NORMAL);
        macros::WHOLE_HIT(agent, 2);
    }
}
"#,
        );
        let imported = import_for_fighter("mario", Track::Game, &original);
        assert!(imported.baseline.is_empty());
        assert!(matches!(
            imported.nodes.as_slice(),
            [MoveNode {
                kind: NodeKind::Execute { commands },
                ..
            }] if matches!(commands.as_slice(), [
                ExcuteStmt::HitStatus { target: crate::data::HurtTarget::Bone(_), status: bone_status },
                ExcuteStmt::HitStatus { target: crate::data::HurtTarget::Group(3), status: group_status },
                ExcuteStmt::HitStatus { target: crate::data::HurtTarget::Whole, status: whole_status },
            ] if bone_status == "HIT_STATUS_XLU" && group_status == "HIT_STATUS_NORMAL" && whole_status == "2")
        ));

        let compiled = compile("mario", "attack11", &graph(imported)).unwrap();
        assert_eq!(compiled[0].0, Track::Game);
        assert_json_eq!(&compiled[0].1, &original);
        let emitted = crate::acmd::preview_game_fn(&compiled[0].1, "attack11");
        assert_json_eq!(crate::acmd::parse_acmd_script(&emitted), &original);

        let arbitrary = crate::acmd::parse_acmd_script(
            r#"unsafe extern "C" fn game_attack11(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::WHOLE_HIT(agent, StatusModule::status_kind(agent.module_accessor));
    }
}
"#,
        );
        let preserved = import_for_fighter("mario", Track::Game, &arbitrary);
        assert!(!preserved.baseline.is_empty());
        assert!(matches!(
            preserved.nodes[0].kind,
            NodeKind::Preserved { .. }
        ));
        let preserved_output = compile("mario", "attack11", &graph(preserved)).unwrap();
        let preserved_export = crate::acmd::preview_game_fn(&preserved_output[0].1, "attack11");
        assert_json_eq!(
            crate::acmd::parse_acmd_script(&preserved_export),
            arbitrary,
            "unmeasured hurt-status expressions remain source-preserved"
        );
    }

    #[test]
    fn fighter_aware_import_rebuilds_only_exact_known_conditionals() {
        let conditions = [
            (
                "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND {",
                Some(MoveCondition::Grounded),
                "bayonetta",
            ),
            (
                "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_AIR {",
                Some(MoveCondition::Airborne),
                "bayonetta",
            ),
            (
                "if MotionModule::is_changing(agent.module_accessor) {",
                Some(MoveCondition::MotionChanging),
                "eflame",
            ),
            (
                "if WorkModule::is_flag(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_AIR_SPECIAL_S_U_TO_D) {",
                Some(MoveCondition::WorkFlag {
                    symbol: "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_AIR_SPECIAL_S_U_TO_D".into(),
                    expected: true,
                }),
                "bayonetta",
            ),
            (
                "if !WorkModule::is_flag(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_AIR_SPECIAL_S_U_TO_D) {",
                Some(MoveCondition::WorkFlag {
                    symbol: "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_AIR_SPECIAL_S_U_TO_D".into(),
                    expected: false,
                }),
                "bayonetta",
            ),
            (
                "if ControlModule::check_button_on(agent.module_accessor, *CONTROL_PAD_BUTTON_ATTACK) {",
                Some(MoveCondition::ButtonHeld {
                    button: InputButton::Attack,
                }),
                "bayonetta",
            ),
            (
                "if ControlModule::check_button_on(agent.module_accessor, *CONTROL_PAD_BUTTON_CATCH) {",
                Some(MoveCondition::ButtonHeld {
                    button: InputButton::Grab,
                }),
                "bayonetta",
            ),
        ];
        for (header, expected, fighter) in conditions {
            assert_json_eq!(import_condition(Some(fighter), header), &expected);
            let source = AcmdScript {
                stmts: vec![
                    AcmdStmt::RawBlock {
                        header: header.into(),
                        body: vec![AcmdStmt::Wait(1.0)],
                    },
                    AcmdStmt::RawBlock {
                        header: "else {".into(),
                        body: vec![AcmdStmt::Excute(vec![ExcuteStmt::ClearAll])],
                    },
                ],
            };
            let imported = import_for_fighter(fighter, Track::Game, &source);
            assert_eq!(imported.nodes.len(), 1);
            let NodeKind::Branch {
                condition,
                then_nodes,
                else_nodes,
            } = &imported.nodes[0].kind
            else {
                panic!("supported conditional was not imported as a branch: {header}");
            };
            assert_json_eq!(condition, expected.unwrap());
            assert_eq!(then_nodes.len(), 1);
            assert_eq!(else_nodes.len(), 1);
            let compiled = compile(fighter, "attack11", &graph(imported)).unwrap();
            assert_json_eq!(&compiled[0].1, source, "source tree changed for {header}");
        }
    }

    #[test]
    fn motion_changing_condition_is_game_only_source_preserved_and_compound_safe() {
        use crate::move_nodes::model::MoveCondition;

        let source = crate::acmd::parse_acmd_script(
            r#"unsafe extern "C" fn game_motion_changing(agent: &mut L2CAgentBase) {
    if MotionModule::is_changing(agent.module_accessor) {
        frame(agent.lua_state_agent, 1.0);
    }
}
"#,
        );
        for track in [Track::Game, Track::Expression, Track::Sound, Track::Effect] {
            let imported = import_for_fighter("eflame", track, &source);
            if track == Track::Game {
                assert!(matches!(
                    imported.nodes.as_slice(),
                    [MoveNode {
                        kind: NodeKind::Branch {
                            condition: MoveCondition::MotionChanging,
                            ..
                        },
                        ..
                    }]
                ));
                let authored = graph(imported.clone());
                assert!(super::super::live_support::source_only_tracks(&authored)
                    .any(|blocked| blocked == Track::Game));
                let compiled = compile("eflame", "attackdash", &authored).unwrap();
                let emitted = crate::acmd::preview_game_fn(&compiled[0].1, "attackdash");
                assert_json_eq!(
                    crate::acmd::parse_acmd_script(&emitted).stmts,
                    &source.stmts
                );
            } else {
                assert!(
                    matches!(
                        imported.nodes.as_slice(),
                        [MoveNode {
                            kind: NodeKind::Preserved { .. },
                            ..
                        }]
                    ),
                    "MotionModule condition became editable on {track:?}"
                );
                let compiled = compile("eflame", "attackdash", &graph(imported)).unwrap();
                assert_json_eq!(&compiled[0].1, &source);
            }
        }

        let invalid_expression = graph(fixture::track(
            Track::Expression,
            vec![fixture::node(
                1,
                NodeKind::Branch {
                    condition: MoveCondition::Not(Box::new(MoveCondition::MotionChanging)),
                    then_nodes: vec![],
                    else_nodes: vec![],
                },
            )],
        ));
        assert!(compile("eflame", "attackdash", &invalid_expression)
            .unwrap_err()
            .iter()
            .any(|error| error.reason.contains("only on the Game track")));
    }

    #[test]
    fn status_and_item_kind_conditions_are_owner_bound_and_round_trip() {
        let cases = [
            ("cloud", "if StatusModule::status_kind(agent.module_accessor) == *FIGHTER_CLOUD_STATUS_KIND_FINAL2 {", MoveCondition::InStatus { symbol: "FIGHTER_CLOUD_STATUS_KIND_FINAL2".into() }),
            ("cloud", "if StatusModule::status_kind(agent.module_accessor) == *FIGHTER_STATUS_KIND_WAIT {", MoveCondition::InStatus { symbol: "FIGHTER_STATUS_KIND_WAIT".into() }),
            ("mario", "if ItemModule::get_have_item_kind(agent.module_accessor, 0) == *ITEM_KIND_BEAMSWORD {", MoveCondition::HoldingItemKind { item: "ITEM_KIND_BEAMSWORD".into() }),
        ];
        for (fighter, header, expected) in cases {
            assert_json_eq!(import_condition(Some(fighter), header), Some(&expected));
            let expression = atomic_condition_expression(fighter, &expected).unwrap();
            assert_eq!(format!("if {expression} {{"), header);
        }
        for (fighter, header) in [
            ("mario", "if StatusModule::status_kind(agent.module_accessor) == *FIGHTER_CLOUD_STATUS_KIND_FINAL2 {"),
            ("cloud", "if StatusModule::status_kind(agent.module_accessor) == *ITEM_KIND_BEAMSWORD {"),
            ("mario", "if ItemModule::get_have_item_kind(agent.module_accessor, 0) == *FIGHTER_STATUS_KIND_WAIT {"),
            ("mario", "if ItemModule::get_have_item_kind(agent.module_accessor, 0) == *ITEM_KIND_NONE {"),
            ("mario", "if ItemModule::get_have_item_kind(agent.module_accessor, 1) == *ITEM_KIND_BEAMSWORD {"),
        ] {
            assert!(import_condition(Some(fighter), header).is_none(), "{header}");
        }
    }

    #[test]
    fn article_exists_is_exact_owner_bound_and_round_trips_across_source_tracks() {
        let symbol = "FIGHTER_SONIC_GENERATE_ARTICLE_GIMMICKJUMP";
        let positive = format!("if macros::IS_EXIST_ARTICLE(agent, *{symbol}) {{");
        let negative = format!("if !macros::IS_EXIST_ARTICLE(agent, *{symbol}) {{");
        let expected = MoveCondition::ArticleExists {
            symbol: symbol.into(),
        };

        assert_json_eq!(import_condition(Some("sonic"), &positive), Some(&expected));
        assert_json_eq!(
            import_condition(Some("sonic"), &negative),
            Some(&MoveCondition::Not(Box::new(expected.clone())))
        );
        assert!(import_condition(Some("mario"), &positive).is_none());
        for malformed in [
            "if macros::IS_EXIST_ARTICLE(boma, *FIGHTER_SONIC_GENERATE_ARTICLE_GIMMICKJUMP) {",
            "if macros::IS_EXIST_ARTICLE(agent, FIGHTER_SONIC_GENERATE_ARTICLE_GIMMICKJUMP) {",
            "if macros::IS_EXIST_ARTICLE(agent, *FIGHTER_INSTANCE_WORK_ID_FLAG_READY_GO) {",
            "if macros::IS_EXIST_ARTICLE(agent, *FIGHTER_SONIC_GENERATE_ARTICLE_GIMMICKJUMP, 0) {",
            "if macros::IS_EXIST_ARTICLE(agent, *FIGHTER_SONIC_GENERATE_ARTICLE_GIMMICKJUMP) && true {",
            "if macros::IS_EXIST_ARTICLE(agent, *FIGHTER_SONIC_GENERATE_ARTICLE_GIMMICKJUMP) { injected() }",
            "if ! macros::IS_EXIST_ARTICLE(agent, *FIGHTER_SONIC_GENERATE_ARTICLE_GIMMICKJUMP) {",
        ] {
            assert!(
                import_condition(Some("sonic"), malformed).is_none(),
                "{malformed}"
            );
        }
        assert!(import_condition(None, &positive).is_none());

        let serialized = serde_json::to_value(&expected).unwrap();
        let mut unknown_field = serialized.clone();
        unknown_field["article_exists"]["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<MoveCondition>(unknown_field).is_err());
        let decoded: MoveCondition = serde_json::from_value(serialized).unwrap();
        assert_json_eq!(&decoded, &expected);
        let wrong_owner_graph = graph(fixture::track(
            Track::Game,
            vec![fixture::node(
                1,
                NodeKind::Branch {
                    condition: expected.clone(),
                    then_nodes: vec![],
                    else_nodes: vec![],
                },
            )],
        ));
        assert!(compile("mario", "attack11", &wrong_owner_graph).is_err());

        for (track, prefix, parse) in [
            (
                Track::Game,
                "game",
                crate::acmd::parse_acmd_script as fn(&str) -> AcmdScript,
            ),
            (
                Track::Expression,
                "expression",
                crate::acmd::parse_expression_script as fn(&str) -> AcmdScript,
            ),
            (
                Track::Sound,
                "sound",
                crate::acmd::parse_sound_script as fn(&str) -> AcmdScript,
            ),
        ] {
            let source = format!(
                r#"unsafe extern "C" fn {prefix}_article_exists_audit(agent: &mut L2CAgentBase) {{
    {positive}
        if macros::is_excute(agent) {{
            ArticleModule::shoot(agent.module_accessor, *FIGHTER_SONIC_GENERATE_ARTICLE_GIMMICKJUMP, *ARTICLE_OPE_TARGET_ALL, false);
        }}
    }}
    {negative}
        if macros::is_excute(agent) {{
            ArticleModule::shoot(agent.module_accessor, *FIGHTER_SONIC_GENERATE_ARTICLE_GIMMICKJUMP, *ARTICLE_OPE_TARGET_LAST, false);
        }}
    }}
}}"#
            );
            let parsed = parse(&source);
            assert_eq!(parsed.stmts.len(), 2, "{track:?}");
            let imported = import_for_fighter("sonic", track, &parsed);
            assert_eq!(imported.nodes.len(), 2, "{track:?}");
            let expected_conditions = [
                expected.clone(),
                MoveCondition::Not(Box::new(expected.clone())),
            ];
            for (node, expected_condition) in imported.nodes.iter().zip(expected_conditions) {
                let NodeKind::Branch { condition, .. } = &node.kind else {
                    panic!("measured article-existence guard was not typed on {track:?}");
                };
                assert_json_eq!(condition, expected_condition, "{track:?}");
            }

            let encoded = serde_json::to_string(&imported).unwrap();
            let imported: MoveTrack = serde_json::from_str(&encoded).unwrap();
            let compiled = compile("sonic", "attack11", &graph(imported)).unwrap();
            let output = &compiled[0].1;
            let mut raw_shoot_count = 0;
            fn count_raw_shoot(stmts: &[AcmdStmt], count: &mut usize) {
                for stmt in stmts {
                    match stmt {
                        AcmdStmt::Excute(commands) => {
                            *count += commands.iter().filter(|command| matches!(
                                command,
                                ExcuteStmt::Raw(raw) if raw.contains("ArticleModule::shoot(")
                            )).count();
                        }
                        AcmdStmt::RawBlock { body, .. } | AcmdStmt::Loop { body, .. } => {
                            count_raw_shoot(body, count);
                        }
                        _ => {}
                    }
                }
            }
            count_raw_shoot(output.stmts.as_slice(), &mut raw_shoot_count);
            assert_eq!(
                raw_shoot_count, 2,
                "nested launch calls must remain opaque on {track:?}"
            );

            let preview = match track {
                Track::Game => crate::acmd::preview_game_fn(output, "attack11"),
                Track::Expression => crate::acmd::preview_expression_fn(output, "attack11"),
                Track::Sound => crate::acmd::preview_sound_fn(output, "attack11"),
                Track::Effect => unreachable!(),
            };
            assert_json_eq!(
                parse(&preview),
                parsed,
                "import/compile/preview/reparse changed {track:?} source"
            );
        }
    }

    #[test]
    fn numeric_native_branches_round_trip_and_edit_without_flattening_control_flow() {
        use crate::move_nodes::model::{NativeComparison, NativeIntOperand};
        for track in [Track::Game, Track::Sound, Track::Expression] {
            let source = AcmdScript {
                stmts: vec![
                    AcmdStmt::RawBlock {
                        header: "if WorkModule::get_int(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_INT_KIND) == *FIGHTER_KIND_KIRBY {".into(),
                        body: vec![AcmdStmt::Wait(1.0), AcmdStmt::RawBlock {
                            header: "if WorkModule::get_int(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_INT_COLOR) != 2 {".into(),
                            body: vec![AcmdStmt::Wait(2.0)],
                        }],
                    },
                    AcmdStmt::RawBlock { header: "else {".into(), body: vec![AcmdStmt::Wait(3.0)] },
                    AcmdStmt::RawBlock { header: "if unreviewed_native_guard(agent) {".into(), body: vec![AcmdStmt::Wait(4.0)] },
                ],
            };
            let imported = import_for_fighter("cloud", track, &source);
            assert!(matches!(imported.nodes[0].kind, NodeKind::Branch { .. }));
            assert_eq!(imported.baseline.len(), 1);
            let mut recipe = graph(imported);
            let encoded = serde_json::to_string(&recipe).unwrap();
            recipe = serde_json::from_str(&encoded).unwrap();
            let compiled = compile("cloud", "attack11", &recipe).unwrap();
            assert_json_eq!(&compiled[0].1, &source);
            let NodeKind::Branch {
                condition,
                then_nodes,
                else_nodes,
            } = &mut recipe.tracks[0].nodes[0].kind
            else {
                panic!()
            };
            assert_eq!(else_nodes.len(), 1);
            let NodeKind::Branch {
                condition: inner, ..
            } = &then_nodes[1].kind
            else {
                panic!()
            };
            assert!(matches!(
                inner,
                MoveCondition::WorkInt {
                    op: NativeComparison::Ne,
                    value: NativeIntOperand::Literal(2),
                    ..
                }
            ));
            *condition = MoveCondition::WorkFloat {
                symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".into(),
                op: NativeComparison::Gte,
                value: 87.5,
            };
            let edited = compile("cloud", "attack11", &recipe).unwrap();
            let AcmdStmt::RawBlock { header, body } = &edited[0].1.stmts[0] else {
                panic!()
            };
            assert_eq!(
                header,
                "if WorkModule::get_float(agent.module_accessor, *FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE) >= 87.5 {"
            );
            let AcmdStmt::RawBlock {
                body: original_body,
                ..
            } = &source.stmts[0]
            else {
                panic!()
            };
            assert_json_eq!(body, original_body);
            assert_json_eq!(&edited[0].1.stmts[1..], &source.stmts[1..]);
            assert!(super::super::live_support::source_only_tracks(&recipe)
                .any(|blocked| blocked == track));
        }
    }

    #[test]
    fn numeric_native_branches_reject_foreign_sources_and_unreviewed_source_stays_opaque() {
        use crate::move_nodes::model::{NativeComparison, NativeIntOperand};
        for condition in [
            MoveCondition::WorkInt {
                symbol: "FIGHTER_RIDLEY_INSTANCE_WORK_ID_INT_DISABLE_SPECIAL_LW_FINISH_COUNT"
                    .into(),
                op: NativeComparison::Eq,
                value: NativeIntOperand::Literal(0),
            },
            MoveCondition::WorkFloat {
                symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".into(),
                op: NativeComparison::Eq,
                value: f32::NAN,
            },
            MoveCondition::WorkInt {
                symbol: "FIGHTER_INSTANCE_WORK_ID_INT_COLOR".into(),
                op: NativeComparison::Eq,
                value: NativeIntOperand::FighterKind("not_a_kind".into()),
            },
        ] {
            let track = fixture::track(
                Track::Game,
                vec![fixture::node(
                    1,
                    NodeKind::Branch {
                        condition,
                        then_nodes: vec![],
                        else_nodes: vec![],
                    },
                )],
            );
            assert!(compile("cloud", "attack11", &graph(track)).is_err());
        }
        for header in [
            "if WorkModule::get_int(agent.module_accessor, *FIGHTER_IKE_STATUS_SPECIAL_N_WORK_INT_READY_LEVEL) == 0 {",
            "if WorkModule::get_int(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_INT_KIND) == *FIGHTER_KIND_TERM {",
            "if WorkModule::get_int(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_INT_COLOR) == 0 && extra(agent) {",
            "if WorkModule::get_float(agent.module_accessor, *FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE) >= 1e2 {",
        ] {
            let source = AcmdScript {
                stmts: vec![AcmdStmt::RawBlock {
                    header: header.into(),
                    body: vec![AcmdStmt::Wait(1.0)],
                }],
            };
            let imported = import_for_fighter("cloud", Track::Game, &source);
            assert!(fixture::untyped_condition(&imported.nodes[0].kind), "{header}");
            assert_json_eq!(
                &compile("cloud", "attack11", &graph(imported)).unwrap()[0].1,
                source
            );
        }
    }

    #[test]
    fn source_bound_integer_conditions_round_trip_across_tracks_and_edits() {
        use crate::move_nodes::model::{NativeComparison, NativeIntOperand};

        let tracks = [Track::Game, Track::Effect, Track::Sound, Track::Expression];
        let costume_source = "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_COSTUME_KIND";
        let costume_choices =
            super::super::native_conditions::integer_choices("bayonetta", costume_source);
        assert_eq!(costume_choices.len(), 3);
        for (choice_index, choice) in costume_choices.iter().enumerate() {
            for track in tracks {
                let header = format!(
                    "if WorkModule::get_int(agent.module_accessor, *{costume_source}) != *{} {{",
                    choice.symbol
                );
                let source = AcmdScript {
                    stmts: vec![
                        AcmdStmt::Frame(0.5),
                        AcmdStmt::RawBlock {
                            header: header.clone(),
                            body: vec![AcmdStmt::Wait(1.0)],
                        },
                        AcmdStmt::RawBlock {
                            header: "else {".into(),
                            body: vec![AcmdStmt::Wait(2.0)],
                        },
                        AcmdStmt::RawBlock {
                            header: "if unknown_neighbor(agent) {".into(),
                            body: vec![AcmdStmt::Wait(3.0)],
                        },
                    ],
                };
                let imported = import_for_fighter("bayonetta", track, &source);
                assert_eq!(imported.nodes.len(), 3, "{track:?}: {header}");
                assert!(matches!(imported.nodes[1].kind, NodeKind::Branch { .. }));
                assert_eq!(imported.baseline.len(), 1);

                let mut recipe: MoveGraph =
                    serde_json::from_value(serde_json::to_value(graph(imported)).unwrap()).unwrap();
                let unchanged = compile("bayonetta", "attack11", &recipe).unwrap();
                assert_eq!(unchanged[0].0, track);
                assert_json_eq!(
                    &unchanged[0].1,
                    &source,
                    "{track:?}: imported source changed for {header}"
                );

                let NodeKind::Branch {
                    condition,
                    then_nodes,
                    else_nodes,
                } = &mut recipe.tracks[0].nodes[1].kind
                else {
                    panic!("native condition was not imported: {header}");
                };
                assert!(matches!(
                    condition,
                    MoveCondition::WorkInt {
                        op: NativeComparison::Ne,
                        value: NativeIntOperand::Choice(symbol),
                        ..
                    } if symbol == choice.symbol
                ));
                assert!(matches!(
                    then_nodes.as_slice(),
                    [MoveNode {
                        kind: NodeKind::Wait { frames: 1.0 },
                        ..
                    }]
                ));
                assert!(matches!(
                    else_nodes.as_slice(),
                    [MoveNode {
                        kind: NodeKind::Wait { frames: 2.0 },
                        ..
                    }]
                ));

                let edited_choice = costume_choices[(choice_index + 1) % costume_choices.len()];
                *condition = MoveCondition::WorkInt {
                    symbol: costume_source.into(),
                    op: NativeComparison::Eq,
                    value: NativeIntOperand::Choice(edited_choice.symbol.into()),
                };
                let edited_recipe: MoveGraph =
                    serde_json::from_value(serde_json::to_value(recipe).unwrap()).unwrap();
                let edited = compile("bayonetta", "attack11", &edited_recipe).unwrap();
                let expected_header = format!(
                    "if WorkModule::get_int(agent.module_accessor, *{costume_source}) == *{} {{",
                    edited_choice.symbol
                );
                let AcmdStmt::RawBlock {
                    header: emitted_header,
                    body: then_body,
                } = &edited[0].1.stmts[1]
                else {
                    panic!("edited branch was not emitted as a raw block");
                };
                assert_eq!(emitted_header, &expected_header);
                assert_json_eq!(then_body, vec![AcmdStmt::Wait(1.0)]);
                assert!(matches!(
                    &edited[0].1.stmts[2],
                    AcmdStmt::RawBlock { header, .. } if header == "else {"
                ));
                assert_json_eq!(
                    &edited[0].1.stmts[2],
                    &AcmdStmt::RawBlock {
                        header: "else {".into(),
                        body: vec![AcmdStmt::Wait(2.0)],
                    }
                );
                assert_json_eq!(&edited[0].1.stmts[3], &source.stmts[3]);
                let exported = preview(track, &edited[0].1, "attack11");
                syn::parse_file(&exported).expect("edited track preview should parse");
                assert!(exported.contains(&expected_header));
            }
        }

        for (fighter, source_symbol, rhs) in [
            (
                "ike",
                "FIGHTER_IKE_STATUS_SPECIAL_N_WORK_INT_READY_LEVEL",
                1,
            ),
            (
                "roy",
                "FIGHTER_ROY_STATUS_SPECIAL_N_WORK_INT_READY_LEVEL",
                2,
            ),
        ] {
            for track in tracks {
                let header = format!(
                    "if WorkModule::get_int(agent.module_accessor, *{source_symbol}) == {rhs} {{"
                );
                let source = AcmdScript {
                    stmts: vec![
                        AcmdStmt::Wait(0.5),
                        AcmdStmt::RawBlock {
                            header: header.clone(),
                            body: vec![AcmdStmt::Frame(1.0)],
                        },
                        AcmdStmt::RawBlock {
                            header: "else {".into(),
                            body: vec![AcmdStmt::Wait(2.0)],
                        },
                        AcmdStmt::RawBlock {
                            header: "if unknown_neighbor(agent) {".into(),
                            body: vec![AcmdStmt::Wait(3.0)],
                        },
                    ],
                };
                let imported = import_for_fighter(fighter, track, &source);
                assert_eq!(imported.nodes.len(), 3, "{fighter}/{track:?}: {header}");
                assert!(matches!(imported.nodes[1].kind, NodeKind::Branch { .. }));
                assert_eq!(imported.baseline.len(), 1);
                let mut recipe: MoveGraph =
                    serde_json::from_value(serde_json::to_value(graph(imported)).unwrap()).unwrap();
                let unchanged = compile(fighter, "attack11", &recipe).unwrap();
                assert_eq!(unchanged[0].0, track);
                assert_json_eq!(
                    &unchanged[0].1,
                    &source,
                    "{fighter}/{track:?}: imported source changed for {header}"
                );

                let NodeKind::Branch {
                    condition,
                    then_nodes,
                    else_nodes,
                } = &mut recipe.tracks[0].nodes[1].kind
                else {
                    panic!("native condition was not imported: {header}");
                };
                assert!(matches!(
                    condition,
                    MoveCondition::WorkInt {
                        op: NativeComparison::Eq,
                        value: NativeIntOperand::Literal(value),
                        ..
                    } if *value == rhs
                ));
                assert!(matches!(
                    then_nodes.as_slice(),
                    [MoveNode {
                        kind: NodeKind::Frame { frame: 1.0 },
                        ..
                    }]
                ));
                assert!(matches!(
                    else_nodes.as_slice(),
                    [MoveNode {
                        kind: NodeKind::Wait { frames: 2.0 },
                        ..
                    }]
                ));

                *condition = MoveCondition::WorkInt {
                    symbol: source_symbol.into(),
                    op: NativeComparison::Ne,
                    value: NativeIntOperand::Literal(rhs + 1),
                };
                let edited_recipe: MoveGraph =
                    serde_json::from_value(serde_json::to_value(recipe).unwrap()).unwrap();
                let edited = compile(fighter, "attack11", &edited_recipe).unwrap();
                let expected_header = format!(
                    "if WorkModule::get_int(agent.module_accessor, *{source_symbol}) != {} {{",
                    rhs + 1
                );
                let AcmdStmt::RawBlock {
                    header: emitted_header,
                    body: then_body,
                } = &edited[0].1.stmts[1]
                else {
                    panic!("edited branch was not emitted as a raw block");
                };
                assert_eq!(emitted_header, &expected_header);
                assert_json_eq!(then_body, vec![AcmdStmt::Frame(1.0)]);
                assert!(matches!(
                    &edited[0].1.stmts[2],
                    AcmdStmt::RawBlock { header, .. } if header == "else {"
                ));
                assert_json_eq!(
                    &edited[0].1.stmts[2],
                    &AcmdStmt::RawBlock {
                        header: "else {".into(),
                        body: vec![AcmdStmt::Wait(2.0)],
                    }
                );
                assert_json_eq!(&edited[0].1.stmts[3], &source.stmts[3]);
                let exported = preview(track, &edited[0].1, "attack11");
                syn::parse_file(&exported).expect("edited track preview should parse");
                assert!(exported.contains(&expected_header));
            }
        }
    }

    #[test]
    fn native_condition_status_reads_do_not_authorize_native_setters() {
        for fighter in ["ike", "roy"] {
            for symbol in super::super::native_conditions::status_int_reads(fighter) {
                assert!(validate_registered_work_symbol(
                    fighter,
                    &format!("*{symbol}"),
                    IdentifierKind::WorkInt
                )
                .is_err());
            }
        }
    }

    #[test]
    fn source_bound_integer_condition_rejections_stay_opaque_or_fail_authored_compile() {
        use crate::move_nodes::model::{NativeComparison, NativeIntOperand};

        let rejected = [
            (
                "bayonetta",
                "if WorkModule::get_int(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_INT_COLOR) == *FIGHTER_BAYONETTA_COSTUME_KIND_DEFAULT {",
            ),
            (
                "ike",
                "if WorkModule::get_int(agent.module_accessor, *FIGHTER_ROY_STATUS_SPECIAL_N_WORK_INT_READY_LEVEL) == 0 {",
            ),
            (
                "roy",
                "if WorkModule::get_int(agent.module_accessor, *FIGHTER_IKE_STATUS_SPECIAL_N_WORK_INT_READY_LEVEL) == 0 {",
            ),
            (
                "bayonetta",
                "if WorkModule::get_int(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_COSTUME_KIND) == *FIGHTER_BAYONETTA_COSTUME_KIND_UNKNOWN {",
            ),
            (
                "ike",
                "if WorkModule::get_int(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_COSTUME_KIND) == *FIGHTER_BAYONETTA_COSTUME_KIND_DEFAULT {",
            ),
            (
                "bayonetta",
                "if WorkModule::get_float(agent.module_accessor, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_COSTUME_KIND) == 1.0 {",
            ),
            (
                "ike",
                "if WorkModule::get_int(agent.module_accessor, *FIGHTER_IKE_STATUS_SPECIAL_N_WORK_INT_READY_LEVEL) == 2147483648 {",
            ),
        ];
        for (fighter, header) in rejected {
            for track in [Track::Game, Track::Effect, Track::Sound, Track::Expression] {
                let source = AcmdScript {
                    stmts: vec![
                        AcmdStmt::RawBlock {
                            header: header.into(),
                            body: vec![AcmdStmt::Wait(1.0)],
                        },
                        AcmdStmt::RawBlock {
                            header: "else {".into(),
                            body: vec![AcmdStmt::Wait(2.0)],
                        },
                    ],
                };
                let imported = import_for_fighter(fighter, track, &source);
                assert!(
                    imported
                        .nodes
                        .iter()
                        .all(|node| fixture::untyped_condition(&node.kind)),
                    "{fighter}/{track:?}: {header}"
                );
                let unchanged = compile(fighter, "attack11", &graph(imported)).unwrap();
                assert_json_eq!(
                    &unchanged[0].1,
                    &source,
                    "{fighter}/{track:?}: opaque source changed for {header}"
                );
            }
        }

        let authored = [
            (
                "bayonetta",
                "FIGHTER_INSTANCE_WORK_ID_INT_COLOR",
                NativeIntOperand::Choice("FIGHTER_BAYONETTA_COSTUME_KIND_DEFAULT".into()),
            ),
            (
                "ike",
                "FIGHTER_ROY_STATUS_SPECIAL_N_WORK_INT_READY_LEVEL",
                NativeIntOperand::Literal(0),
            ),
            (
                "bayonetta",
                "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_COSTUME_KIND",
                NativeIntOperand::Choice("FIGHTER_BAYONETTA_COSTUME_KIND_UNKNOWN".into()),
            ),
            (
                "ike",
                "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_INT_COSTUME_KIND",
                NativeIntOperand::Choice("FIGHTER_BAYONETTA_COSTUME_KIND_DEFAULT".into()),
            ),
        ];
        for (fighter, symbol, value) in authored {
            let recipe = graph(fixture::track(
                Track::Game,
                vec![fixture::node(
                    1,
                    NodeKind::Branch {
                        condition: MoveCondition::WorkInt {
                            symbol: symbol.into(),
                            op: NativeComparison::Eq,
                            value,
                        },
                        then_nodes: vec![],
                        else_nodes: vec![],
                    },
                )],
            ));
            assert!(compile(fighter, "attack11", &recipe).is_err());
        }
        let wrong_type = graph(fixture::track(
            Track::Game,
            vec![fixture::node(
                1,
                NodeKind::Branch {
                    condition: MoveCondition::WorkFloat {
                        symbol: "FIGHTER_IKE_STATUS_SPECIAL_N_WORK_INT_READY_LEVEL".into(),
                        op: NativeComparison::Eq,
                        value: 1.0,
                    },
                    then_nodes: vec![],
                    else_nodes: vec![],
                },
            )],
        ));
        assert!(compile("ike", "attack11", &wrong_type).is_err());
    }

    #[test]
    fn compound_conditions_keep_branches_and_neighbors_across_all_tracks() {
        use crate::move_nodes::model::{NativeComparison, NativeIntOperand};

        let condition = MoveCondition::And {
            left: Box::new(MoveCondition::Grounded),
            right: Box::new(MoveCondition::Group(Box::new(MoveCondition::Or {
                left: Box::new(MoveCondition::ButtonHeld {
                    button: InputButton::Attack,
                }),
                right: Box::new(MoveCondition::Not(Box::new(MoveCondition::WorkFlag {
                    symbol: "FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_AIR_SPECIAL_S_U_TO_D".into(),
                    expected: true,
                }))),
            }))),
        };
        let header = super::super::compound_conditions::header("bayonetta", &condition).unwrap();
        assert!(header.contains(" && ("));
        assert!(header.contains(" || !WorkModule::is_flag"));
        let original = AcmdScript {
            stmts: vec![
                AcmdStmt::Wait(0.5),
                AcmdStmt::RawBlock {
                    header: header.clone(),
                    body: vec![AcmdStmt::Wait(1.0)],
                },
                AcmdStmt::RawBlock {
                    header: "else {".into(),
                    body: vec![AcmdStmt::Frame(2.0)],
                },
                AcmdStmt::RawBlock {
                    header: "if unreviewed_neighbor(agent) {".into(),
                    body: vec![AcmdStmt::Wait(3.0)],
                },
            ],
        };

        for track in [Track::Game, Track::Effect, Track::Sound, Track::Expression] {
            let imported = import_for_fighter("bayonetta", track, &original);
            assert_eq!(imported.nodes.len(), 3, "{track:?}");
            assert_eq!(imported.baseline.len(), 1, "{track:?}");
            let mut recipe: MoveGraph =
                serde_json::from_value(serde_json::to_value(graph(imported)).unwrap()).unwrap();
            let round_trip = compile("bayonetta", "attack11", &recipe).unwrap();
            assert_eq!(round_trip[0].0, track);
            assert_json_eq!(
                &round_trip[0].1,
                &original,
                "imported compound source changed on {track:?}"
            );

            let NodeKind::Branch {
                condition: imported_condition,
                then_nodes,
                else_nodes,
            } = &mut recipe.tracks[0].nodes[1].kind
            else {
                panic!("compound condition was not imported on {track:?}");
            };
            assert_json_eq!(&*imported_condition, &condition);
            assert!(matches!(
                then_nodes.as_slice(),
                [MoveNode {
                    kind: NodeKind::Wait { frames: 1.0 },
                    ..
                }]
            ));
            assert!(matches!(
                else_nodes.as_slice(),
                [MoveNode {
                    kind: NodeKind::Frame { frame: 2.0 },
                    ..
                }]
            ));

            *imported_condition = MoveCondition::Not(Box::new(condition.clone()));
            let edited_recipe: MoveGraph =
                serde_json::from_value(serde_json::to_value(recipe).unwrap()).unwrap();
            let edited = compile("bayonetta", "attack11", &edited_recipe).unwrap();
            let edited_header = super::super::compound_conditions::header(
                "bayonetta",
                &MoveCondition::Not(Box::new(condition.clone())),
            )
            .unwrap();
            assert!(matches!(
                &edited[0].1.stmts[1],
                AcmdStmt::RawBlock { header, .. } if header == &edited_header
            ));
            assert_json_eq!(
                &edited[0].1.stmts[1],
                &AcmdStmt::RawBlock {
                    header: edited_header.clone(),
                    body: vec![AcmdStmt::Wait(1.0)],
                }
            );
            assert_json_eq!(&edited[0].1.stmts[2], &original.stmts[2]);
            assert_json_eq!(&edited[0].1.stmts[3], &original.stmts[3]);
            let preview = super::preview(track, &edited[0].1, "attack11");
            syn::parse_file(&preview).expect("compound preview should parse");
            assert!(preview.contains(&edited_header));
        }

        let unsupported = [
            "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND && WorkModule::is_flag(agent.module_accessor, *FIGHTER_CLOUD_INSTANCE_WORK_ID_FLAG_LIMIT_BREAK) {",
            "if WorkModule::get_int(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_INT_COLOR) == *FIGHTER_BAYONETTA_COSTUME_KIND_DEFAULT && arbitrary(agent) {",
            "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND || unreviewed(agent) {",
        ];
        for source_header in unsupported {
            for track in [Track::Game, Track::Effect, Track::Sound, Track::Expression] {
                let source = AcmdScript {
                    stmts: vec![
                        AcmdStmt::RawBlock {
                            header: source_header.into(),
                            body: vec![AcmdStmt::Wait(1.0)],
                        },
                        AcmdStmt::RawBlock {
                            header: "else {".into(),
                            body: vec![AcmdStmt::Wait(2.0)],
                        },
                    ],
                };
                let imported = import_for_fighter("bayonetta", track, &source);
                assert!(imported
                    .nodes
                    .iter()
                    .all(|node| fixture::untyped_condition(&node.kind)));
                let unchanged = compile("bayonetta", "attack11", &graph(imported)).unwrap();
                assert_json_eq!(
                    &unchanged[0].1,
                    &source,
                    "opaque compound source changed for {source_header} on {track:?}"
                );
            }
        }

        let mismatched_enum = graph(fixture::track(
            Track::Game,
            vec![fixture::node(
                1,
                NodeKind::Branch {
                    condition: MoveCondition::And {
                        left: Box::new(MoveCondition::Grounded),
                        right: Box::new(MoveCondition::WorkInt {
                            symbol: "FIGHTER_INSTANCE_WORK_ID_INT_COLOR".into(),
                            op: NativeComparison::Eq,
                            value: NativeIntOperand::Choice(
                                "FIGHTER_BAYONETTA_COSTUME_KIND_DEFAULT".into(),
                            ),
                        }),
                    },
                    then_nodes: vec![],
                    else_nodes: vec![],
                },
            )],
        ));
        assert!(compile("bayonetta", "attack11", &mismatched_enum).is_err());

        let foreign_flag = graph(fixture::track(
            Track::Game,
            vec![fixture::node(
                1,
                NodeKind::Branch {
                    condition: MoveCondition::Or {
                        left: Box::new(MoveCondition::Grounded),
                        right: Box::new(MoveCondition::WorkFlag {
                            symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLAG_LIMIT_BREAK".into(),
                            expected: true,
                        }),
                    },
                    then_nodes: vec![],
                    else_nodes: vec![],
                },
            )],
        ));
        assert!(compile("bayonetta", "attack11", &foreign_flag).is_err());

        let too_deep = (0..=super::super::compound_conditions::MAX_DEPTH)
            .fold(MoveCondition::Grounded, |condition, _| {
                MoveCondition::Group(Box::new(condition))
            });
        let too_deep = graph(fixture::track(
            Track::Game,
            vec![fixture::node(
                1,
                NodeKind::Branch {
                    condition: too_deep,
                    then_nodes: vec![],
                    else_nodes: vec![],
                },
            )],
        ));
        assert!(compile("mario", "attack11", &too_deep).is_err());

        fn full_condition_tree(depth: usize) -> MoveCondition {
            if depth == 0 {
                MoveCondition::Grounded
            } else {
                MoveCondition::And {
                    left: Box::new(full_condition_tree(depth - 1)),
                    right: Box::new(full_condition_tree(depth - 1)),
                }
            }
        }
        let too_many_nodes = graph(fixture::track(
            Track::Game,
            vec![fixture::node(
                1,
                NodeKind::Branch {
                    condition: full_condition_tree(8),
                    then_nodes: vec![],
                    else_nodes: vec![],
                },
            )],
        ));
        assert!(compile("mario", "attack11", &too_many_nodes).is_err());
    }

    #[test]
    fn unsupported_and_unregistered_conditional_pairs_keep_a_source_header() {
        let cases = [
            (
                "bayonetta",
                "if WorkModule::is_flag(boma, *FIGHTER_BAYONETTA_INSTANCE_WORK_ID_FLAG_AIR_SPECIAL_S_U_TO_D) {",
            ),
            ("mario", "if some_other_condition(agent) {"),
            (
                "mario",
                "if ControlModule::check_button_on(agent.module_accessor, *CONTROL_PAD_BUTTON_ATTACK) && true {",
            ),
        ];
        for (fighter, header) in cases {
            let source = AcmdScript {
                stmts: vec![
                    AcmdStmt::RawBlock {
                        header: header.into(),
                        body: vec![AcmdStmt::Wait(1.0)],
                    },
                    AcmdStmt::RawBlock {
                        header: "else {".into(),
                        body: vec![AcmdStmt::Wait(2.0)],
                    },
                ],
            };
            let imported = import_for_fighter(fighter, Track::Game, &source);
            // The header stays source text, witnessed once; both arms import as typed waits.
            assert!(matches!(
                imported.nodes.as_slice(),
                [MoveNode { kind: NodeKind::Branch { condition: MoveCondition::Source { header: kept }, then_nodes, else_nodes }, .. }]
                    if kept == header
                        && matches!(then_nodes.as_slice(), [MoveNode { kind: NodeKind::Wait { frames }, .. }] if *frames == 1.0)
                        && matches!(else_nodes.as_slice(), [MoveNode { kind: NodeKind::Wait { frames }, .. }] if *frames == 2.0)
            ));
            assert_eq!(imported.baseline.len(), 1);
            let compiled = compile(fighter, "attack11", &graph(imported)).unwrap();
            assert_json_eq!(&compiled[0].1, source);
        }
    }

    #[test]
    fn legacy_import_keeps_control_flow_immutable_without_fighter_context() {
        let source = AcmdScript {
            stmts: vec![
                AcmdStmt::RawBlock {
                    header: "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND {".into(),
                    body: vec![AcmdStmt::Wait(1.0)],
                },
                AcmdStmt::RawBlock {
                    header: "else {".into(),
                    body: vec![AcmdStmt::Wait(2.0)],
                },
            ],
        };
        let imported = import(Track::Game, &source);
        assert_eq!(imported.nodes.len(), 2);
        assert!(imported
            .nodes
            .iter()
            .all(|node| matches!(node.kind, NodeKind::Preserved { .. })));
        assert_eq!(imported.baseline.len(), 2);
        let compiled = compile("mario", "attack11", &graph(imported)).unwrap();
        assert_json_eq!(&compiled[0].1, source);
    }

    #[test]
    fn recognized_condition_does_not_split_empty_else_or_else_if_chains() {
        let condition =
            "if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_GROUND {";
        for stmts in [
            vec![
                AcmdStmt::RawBlock {
                    header: condition.into(),
                    body: vec![AcmdStmt::Wait(1.0)],
                },
                AcmdStmt::RawBlock {
                    header: "else {".into(),
                    body: vec![],
                },
            ],
            vec![
                AcmdStmt::RawBlock {
                    header: condition.into(),
                    body: vec![AcmdStmt::Wait(1.0)],
                },
                AcmdStmt::RawBlock {
                    header: "else if StatusModule::situation_kind(agent.module_accessor) == *SITUATION_KIND_AIR {".into(),
                    body: vec![AcmdStmt::Wait(2.0)],
                },
                AcmdStmt::RawBlock {
                    header: "else {".into(),
                    body: vec![AcmdStmt::Wait(3.0)],
                },
            ],
        ] {
            let source = AcmdScript { stmts };
            let imported = import_for_fighter("mario", Track::Game, &source);
            assert!(imported
                .nodes
                .iter()
                .all(|node| matches!(node.kind, NodeKind::Preserved { .. })));
            assert_eq!(imported.baseline.len(), source.stmts.len());
            let compiled = compile("mario", "attack11", &graph(imported)).unwrap();
            assert_json_eq!(&compiled[0].1, source);
        }
    }

    #[test]
    fn unsupported_schema_and_invalid_timing_are_rejected() {
        let mut graph = MoveGraph {
            version: SCHEMA_VERSION + 1,
            tracks: Vec::new(),
            positions: Default::default(),
        };
        assert!(compile("mario", "attack11", &graph).is_err());
        graph.version = SCHEMA_VERSION;
        graph.tracks.push(fixture::track(
            Track::Game,
            vec![fixture::frame(9, f32::NAN)],
        ));
        assert!(compile("mario", "attack11", &graph).is_err());
    }

    #[test]
    fn duplicate_track_and_graph_wide_node_ids_are_rejected() {
        let graph = MoveGraph {
            version: SCHEMA_VERSION,
            positions: Default::default(),
            tracks: vec![
                fixture::track(Track::Game, vec![fixture::frame(9, 1.0)]),
                fixture::track(Track::Game, vec![fixture::wait(9, 1.0)]),
            ],
        };
        let errors = compile("mario", "attack11", &graph).unwrap_err();
        assert!(errors.iter().any(|error| error.reason == "duplicate track"));
        assert!(errors
            .iter()
            .any(|error| error.reason == "duplicate node id"));
    }

    #[test]
    fn repeat_and_expansion_budgets_are_enforced() {
        let track = fixture::track(
            Track::Game,
            vec![fixture::node(
                1,
                NodeKind::Repeat {
                    motion_loop: crate::data::MotionLoop::No,
                    count: 100,
                    body: vec![fixture::node(
                        2,
                        NodeKind::Repeat {
                            motion_loop: crate::data::MotionLoop::No,
                            count: 100,
                            body: vec![fixture::node(
                                3,
                                NodeKind::Repeat {
                                    motion_loop: crate::data::MotionLoop::No,
                                    count: 100,
                                    body: vec![fixture::wait(4, 0.0)],
                                },
                            )],
                        },
                    )],
                },
            )],
        );
        assert!(compile("mario", "attack11", &graph(track)).is_err());

        // The authored-node expansion formula stays just under its own 100,000 cap here,
        // while the stricter preview witness charges the repeated body pass as well. Final
        // compiled scripts must be rejected before any downstream editor preview can flatten
        // this tree.
        for track in [Track::Game, Track::Effect] {
            let preview_over_budget = fixture::track(
                track,
                vec![fixture::node(
                    10,
                    NodeKind::Repeat {
                        motion_loop: crate::data::MotionLoop::No,
                        count: 46,
                        body: vec![fixture::node(
                            11,
                            NodeKind::Repeat {
                                motion_loop: crate::data::MotionLoop::No,
                                count: 46,
                                body: vec![fixture::node(
                                    12,
                                    NodeKind::Repeat {
                                        motion_loop: crate::data::MotionLoop::No,
                                        count: 46,
                                        body: vec![fixture::wait(13, 0.0)],
                                    },
                                )],
                            },
                        )],
                    },
                )],
            );
            let errors =
                compile("mario", "preview_budget", &graph(preview_over_budget)).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|error| error.reason.contains("preview expansion")),
                "compiled {track:?} script exceeded the preview budget"
            );
        }

        let zero = fixture::track(
            Track::Game,
            vec![fixture::node(
                1,
                NodeKind::Repeat {
                    motion_loop: crate::data::MotionLoop::No,
                    count: 0,
                    body: vec![],
                },
            )],
        );
        assert!(compile("mario", "attack11", &graph(zero)).is_err());
    }

    #[test]
    fn typed_branches_lower_to_nested_if_else_without_flattening() {
        let track = fixture::track(
            Track::Game,
            vec![fixture::node(
                1,
                NodeKind::Branch {
                    condition: MoveCondition::Grounded,
                    then_nodes: vec![
                        fixture::wait(2, 1.0),
                        fixture::node(
                            3,
                            NodeKind::Branch {
                                condition: MoveCondition::ButtonHeld {
                                    button: InputButton::Attack,
                                },
                                then_nodes: vec![fixture::execute(4, vec![attack()])],
                                else_nodes: vec![fixture::wait(5, 0.5)],
                            },
                        ),
                    ],
                    else_nodes: vec![
                        fixture::wait(6, 2.0),
                        fixture::execute(7, vec![ExcuteStmt::ClearAll]),
                    ],
                },
            )],
        );
        let compiled = compile("cloud", "attack11", &graph(track)).unwrap();
        let AcmdStmt::RawBlock { header, body } = &compiled[0].1.stmts[0] else {
            panic!("branch was not emitted as a source block")
        };
        assert!(header.contains("SITUATION_KIND_GROUND"));
        assert!(matches!(body.first(), Some(AcmdStmt::Wait(1.0))));
        assert!(
            matches!(body.get(1), Some(AcmdStmt::RawBlock { header, .. }) if header.contains("CONTROL_PAD_BUTTON_ATTACK"))
        );
        assert!(
            matches!(body.get(2), Some(AcmdStmt::RawBlock { header, .. }) if header == "else {")
        );
        assert!(matches!(
            compiled[0].1.stmts.get(1),
            Some(AcmdStmt::RawBlock { header, .. }) if header == "else {"
        ));
        let source = crate::acmd::preview_game_fn(&compiled[0].1, "attack11");
        let file = syn::parse_file(&source).expect("nested if/else should parse");
        let syn::Item::Fn(function) = &file.items[0] else {
            panic!("preview is not a function")
        };
        let syn::Stmt::Expr(syn::Expr::If(outer_if), _) = &function.block.stmts[0] else {
            panic!("outer branch was not emitted as an if expression")
        };
        assert!(outer_if.else_branch.is_some());
        let syn::Stmt::Expr(syn::Expr::If(inner_if), _) = &outer_if.then_branch.stmts[1] else {
            panic!("nested branch was not emitted inside the then arm")
        };
        assert!(inner_if.else_branch.is_some());
    }

    #[test]
    fn authored_attack_allows_absent_capsule_and_rejects_nonfinite_numeric_fields() {
        let track = fixture::track(Track::Game, vec![fixture::execute(1, vec![attack()])]);
        assert!(compile("cloud", "attack11", &graph(track)).is_ok());

        let mut command = attack();
        let ExcuteStmt::Attack(ref mut call) = command else {
            unreachable!();
        };
        call.damage = f32::NAN;
        let nonfinite = fixture::track(Track::Game, vec![fixture::execute(1, vec![command])]);
        assert!(compile("cloud", "attack11", &graph(nonfinite)).is_err());
    }

    #[test]
    fn branch_conditions_use_sdk_buttons_and_fighter_owned_flags_only() {
        let buttons = [
            (InputButton::Attack, "CONTROL_PAD_BUTTON_ATTACK"),
            (InputButton::Special, "CONTROL_PAD_BUTTON_SPECIAL"),
            (InputButton::Jump, "CONTROL_PAD_BUTTON_JUMP"),
            (InputButton::Guard, "CONTROL_PAD_BUTTON_GUARD"),
            (InputButton::Grab, "CONTROL_PAD_BUTTON_CATCH"),
        ];
        for (index, (button, constant)) in buttons.into_iter().enumerate() {
            let track = fixture::track(
                Track::Game,
                vec![fixture::node(
                    index as u32 + 1,
                    NodeKind::Branch {
                        condition: MoveCondition::ButtonHeld { button },
                        then_nodes: vec![],
                        else_nodes: vec![],
                    },
                )],
            );
            let compiled = compile("cloud", "attack11", &graph(track)).unwrap();
            let source = crate::acmd::preview_game_fn(&compiled[0].1, "attack11");
            assert!(source.contains(constant));
            assert!(syn::parse_file(&source).is_ok());
        }

        let flag_track = fixture::track(
            Track::Game,
            vec![fixture::node(
                1,
                NodeKind::Branch {
                    condition: MoveCondition::WorkFlag {
                        symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLAG_LIMIT_BREAK".into(),
                        expected: false,
                    },
                    then_nodes: vec![],
                    else_nodes: vec![],
                },
            )],
        );
        let compiled = compile("cloud", "attack11", &graph(flag_track.clone())).unwrap();
        let source = crate::acmd::preview_game_fn(&compiled[0].1, "attack11");
        assert!(source.contains("!WorkModule::is_flag"));
        assert!(syn::parse_file(&source).is_ok());

        let mut wrong_fighter = flag_track.clone();
        wrong_fighter.nodes[0].kind = NodeKind::Branch {
            condition: MoveCondition::WorkFlag {
                symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLAG_LIMIT_BREAK".into(),
                expected: true,
            },
            then_nodes: vec![],
            else_nodes: vec![],
        };
        assert!(compile("mario", "attack11", &graph(wrong_fighter)).is_err());
        let mut numeric_escape = flag_track;
        numeric_escape.nodes[0].kind = NodeKind::Branch {
            condition: MoveCondition::WorkFlag {
                symbol: "0x200000e0".into(),
                expected: true,
            },
            then_nodes: vec![],
            else_nodes: vec![],
        };
        assert!(compile("cloud", "attack11", &graph(numeric_escape)).is_err());
    }

    #[test]
    fn imported_tracks_use_disjoint_id_ranges() {
        let script = AcmdScript {
            stmts: vec![AcmdStmt::Wait(1.0)],
        };
        let ids = [Track::Game, Track::Sound, Track::Expression]
            .into_iter()
            .map(|track| import(track, &script).nodes[0].id)
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn measured_direct_rumble_round_trips_as_an_expression_track_command() {
        let original = crate::acmd::parse_expression_script(
            r#"unsafe extern "C" fn expression_attackdash(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        ControlModule::set_rumble(agent.module_accessor, Hash40::new("rbkind_elecattack"), 0, true, *BATTLE_OBJECT_ID_INVALID as u32);
    }
}
"#,
        );
        assert_eq!(original.stmts.len(), 1);
        let imported = import_for_fighter("kirby", Track::Expression, &original);
        assert!(imported.baseline.is_empty());
        assert!(matches!(
            imported.nodes[0].kind,
            NodeKind::Execute { ref commands }
                if matches!(commands.as_slice(), [ExcuteStmt::Expression(
                    crate::data::ExpressionCall::ControlModuleSetRumble { .. }
                )])
        ));

        let compiled = compile("kirby", "attackdash", &graph(imported)).unwrap();
        assert_eq!(compiled[0].0, Track::Expression);
        assert_json_eq!(&compiled[0].1, &original);
        let emitted = crate::acmd::preview_expression_fn(&compiled[0].1, "attackdash");
        assert!(emitted.contains("*BATTLE_OBJECT_ID_INVALID as u32"));
        assert_json_eq!(crate::acmd::parse_expression_script(&emitted), &original);

        let mut edited = import_for_fighter("kirby", Track::Expression, &original);
        let NodeKind::Execute { commands } = &mut edited.nodes[0].kind else {
            panic!("measured rumble should be an editable execute command");
        };
        let ExcuteStmt::Expression(crate::data::ExpressionCall::ControlModuleSetRumble {
            kind,
            duration,
            looped,
            ..
        }) = &mut commands[0]
        else {
            panic!("measured command should retain its direct rumble type");
        };
        *kind = "Hash40::new(\"rbkind_attackm\")".into();
        *duration = "1".into();
        *looped = "false".into();
        let edited = compile("kirby", "attackdash", &graph(edited)).unwrap();
        assert_eq!(edited[0].0, Track::Expression);
        let edited_export = crate::acmd::preview_expression_fn(&edited[0].1, "attackdash");
        let reparsed = crate::acmd::parse_expression_script(&edited_export);
        assert_json_eq!(
            &edited[0].1,
            reparsed,
            "edited native rumble payloads must retain the Expression category and typed arguments"
        );
    }

    #[test]
    fn direct_rumble_validation_is_signature_typed_and_target_closed() {
        use crate::data::ExpressionCall::ControlModuleSetRumble;

        let call = |kind: &str, duration: &str, looped: &str, target: &str| {
            ExcuteStmt::Expression(ControlModuleSetRumble {
                receiver: "agent.module_accessor".into(),
                kind: kind.into(),
                duration: duration.into(),
                looped: looped.into(),
                target: target.into(),
            })
        };
        assert!(validate_command(
            None,
            &call(
                "Hash40::new(\"rbkind_attackm\")",
                "0",
                "false",
                MEASURED_DIRECT_RUMBLE_TARGET,
            )
        )
        .is_ok());

        let mut invalid_receiver = call(
            "Hash40::new(\"rbkind_attackm\")",
            "0",
            "false",
            MEASURED_DIRECT_RUMBLE_TARGET,
        );
        if let ExcuteStmt::Expression(ControlModuleSetRumble { receiver, .. }) =
            &mut invalid_receiver
        {
            *receiver = "arbitrary_pointer".into();
        }
        assert!(validate_command(None, &invalid_receiver).is_err());

        for invalid in [
            call("some_call()", "0", "false", MEASURED_DIRECT_RUMBLE_TARGET),
            call(
                "Hash40::new(\"rbkind_attackm\")",
                "-1",
                "false",
                MEASURED_DIRECT_RUMBLE_TARGET,
            ),
            call(
                "Hash40::new(\"rbkind_attackm\")",
                "1.5",
                "false",
                MEASURED_DIRECT_RUMBLE_TARGET,
            ),
            call(
                "Hash40::new(\"rbkind_attackm\")",
                "0",
                "maybe",
                MEASURED_DIRECT_RUMBLE_TARGET,
            ),
            call(
                "Hash40::new(\"rbkind_attackm\")",
                "0",
                "false",
                "get_target() as u32",
            ),
            call("Hash40::new(\"rbkind_attackm\")", "0", "false", "0 as u32"),
        ] {
            assert!(
                validate_command(None, &invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
        assert!(validate_command(
            None,
            &ExcuteStmt::Expression(crate::data::ExpressionCall::RumbleHit {
                kind: "arbitrary_kind() as u64".into(),
                unk: "0".into(),
            })
        )
        .is_err());
    }

    #[test]
    fn article_partial_motion_is_owner_checked_game_only_and_preserves_wrappers() {
        use crate::data::{ArticleLifecycleCall, ArticlePartialMotionPreset, ExcuteStmt};

        let preset = ArticlePartialMotionPreset::EflameOpen5;
        let line = preset.source_line();
        let source = format!(
            r#"unsafe extern "C" fn game_partial_guard(agent: &mut L2CAgentBase) {{
    if macros::IS_EXIST_ARTICLE(agent, *FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD) {{
        if macros::is_excute(agent) {{
            {line}
        }}
    }}
    if unreviewed_guard(agent) {{
        if macros::is_excute(agent) {{
            {line}
        }}
    }}
    for _ in 0..2 {{
        if macros::is_excute(agent) {{
            {line}
        }}
    }}
}}"#
        );
        let parsed = crate::acmd::parse_acmd_script(&source);
        let imported = import_for_fighter("eflame", Track::Game, &parsed);
        assert!(matches!(
            imported.nodes.as_slice(),
            [
                MoveNode { kind: NodeKind::Branch { then_nodes, .. }, .. },
                MoveNode { kind: NodeKind::Branch { condition: MoveCondition::Source { .. }, .. }, .. },
                MoveNode { kind: NodeKind::Repeat { body, .. }, .. },
            ] if matches!(then_nodes.as_slice(), [MoveNode { kind: NodeKind::Execute { commands }, .. }]
                if matches!(commands.as_slice(), [ExcuteStmt::ArticleLifecycle(
                    ArticleLifecycleCall::AddMotionPartial { preset: actual }
                )] if *actual == preset))
                && matches!(body.as_slice(), [MoveNode { kind: NodeKind::Execute { commands }, .. }]
                    if matches!(commands.as_slice(), [ExcuteStmt::ArticleLifecycle(
                        ArticleLifecycleCall::AddMotionPartial { preset: actual }
                    )] if *actual == preset))
        ));
        let compiled = compile("eflame", "attack11", &graph(imported)).unwrap();
        assert_json_eq!(
            &compiled[0].1,
            &parsed,
            "branch, opaque wrapper, and bounded repeat preserve their complete source tree"
        );

        let wrong_guard = source.replace(
            "FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD",
            "FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD",
        );
        let wrong_guard = crate::acmd::parse_acmd_script(&wrong_guard);
        let preserved = import_for_fighter("eflame", Track::Game, &wrong_guard);
        assert!(fixture::untyped_condition(&preserved.nodes[0].kind));
        assert!(!preserved.baseline.is_empty());

        let direct = AcmdScript {
            stmts: vec![AcmdStmt::Excute(vec![ExcuteStmt::ArticleLifecycle(
                ArticleLifecycleCall::AddMotionPartial { preset },
            )])],
        };
        let wrong_owner = import_for_fighter("elight", Track::Game, &direct);
        assert!(matches!(
            wrong_owner.nodes.first().map(|node| &node.kind),
            Some(NodeKind::Preserved { .. })
        ));
        assert!(compile("elight", "attack11", &graph(wrong_owner)).is_ok());

        let authored_expression = graph(fixture::track(
            Track::Expression,
            vec![fixture::execute(
                1,
                vec![ExcuteStmt::ArticleLifecycle(
                    ArticleLifecycleCall::AddMotionPartial { preset },
                )],
            )],
        ));
        assert!(compile("eflame", "attack11", &authored_expression).is_err());
    }

    #[test]
    fn unmeasured_direct_rumble_target_stays_preserved_in_its_expression_category() {
        let original = crate::acmd::parse_expression_script(
            r#"unsafe extern "C" fn expression_attackdash(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        ControlModule::set_rumble(agent.module_accessor, Hash40::new("rbkind_elecattack"), 0, true, arbitrary_target() as u32);
    }
}
"#,
        );
        let imported = import_for_fighter("kirby", Track::Expression, &original);
        assert!(!imported.baseline.is_empty());
        assert!(matches!(imported.nodes[0].kind, NodeKind::Preserved { .. }));
        let compiled = compile("kirby", "attackdash", &graph(imported)).unwrap();
        assert_eq!(compiled[0].0, Track::Expression);
        assert_json_eq!(&compiled[0].1, &original);
        let emitted = crate::acmd::preview_expression_fn(&compiled[0].1, "attackdash");
        assert_json_eq!(crate::acmd::parse_expression_script(&emitted), original);
    }
}

fn stick_read(axis: super::model::StickAxis, relative_to_facing: bool) -> String {
    let axis = match axis {
        super::model::StickAxis::X => "x",
        super::model::StickAxis::Y => "y",
    };
    let read = format!("ControlModule::get_stick_{axis}(agent.module_accessor)");
    if relative_to_facing {
        format!("({read} * PostureModule::lr(agent.module_accessor))")
    } else {
        read
    }
}

#[cfg(test)]
mod stick_condition_tests {
    use super::*;
    use crate::move_nodes::model::{NativeComparison, StickAxis};

    #[test]
    fn stick_comparisons_round_trip_exact_control_reads_and_reject_invalid_forms() {
        for (axis, relative_to_facing) in [
            (StickAxis::X, false),
            (StickAxis::X, true),
            (StickAxis::Y, false),
        ] {
            for op in NativeComparison::ALL {
                let condition = MoveCondition::Stick {
                    axis,
                    op,
                    value: -0.5,
                    relative_to_facing,
                };
                let read = atomic_condition_expression("robot", &condition).unwrap();
                let header = format!("if {read} {{");
                let parsed = import_condition(Some("robot"), &header).unwrap();
                assert_eq!(
                    serde_json::to_value(parsed).unwrap(),
                    serde_json::to_value(&condition).unwrap()
                );
                assert_eq!(read.contains("PostureModule::lr"), relative_to_facing);
                let source=crate::acmd::parse_acmd_script(&format!("unsafe extern \"C\" fn game_attack11(agent: &mut L2CAgentBase) {{\n{header}\n    wait(agent.lua_state_agent, 1.0);\n}}\n}}"));
                let track = import_for_fighter("robot", Track::Game, &source);
                assert!(matches!(track.nodes[0].kind, NodeKind::Branch { .. }));
                let graph = MoveGraph {
                    tracks: vec![track],
                    ..Default::default()
                };
                let compiled = compile("robot", "attack11", &graph).unwrap();
                assert_eq!(
                    serde_json::to_value(&compiled[0].1).unwrap(),
                    serde_json::to_value(&source).unwrap()
                );
            }
        }
        for (axis, value, relative_to_facing) in [
            (StickAxis::X, f32::NAN, false),
            (StickAxis::X, 1.01, false),
            (StickAxis::Y, 0.5, true),
        ] {
            assert!(atomic_condition_expression(
                "robot",
                &MoveCondition::Stick {
                    axis,
                    op: NativeComparison::Gt,
                    value,
                    relative_to_facing
                }
            )
            .is_err());
        }
        for bad in [
            "if ControlModule::get_stick_x(other.module_accessor) > 0.5 {",
            "if ControlModule::get_stick_x(agent.module_accessor) > threshold {",
            "if ControlModule::get_stick_x(agent.module_accessor) > 1.01 {",
            "if ControlModule::get_stick_y(agent.module_accessor, 0) > 0.5 {",
        ] {
            assert!(import_condition(Some("robot"), bad).is_none(), "{bad}");
        }
    }
}
