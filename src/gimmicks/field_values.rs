//! Bounded addresses and typed edits for nested behavior values and guards.
//!
//! Expressions and predicates stay nested in the canonical `BehaviorGraph`.
//! These addresses select existing nodes; edits replace only the selected
//! variable reference and pass the full graph through `graph_edit` validation.

use super::behavior::{Action, BehaviorGraph, ConstValue, Expr, Predicate, VarType};
use super::field_projection;
use super::graph_edit;
use super::hooks::HookName;
use super::live_model;

const MAX_PATH_DEPTH: usize = live_model::MAX_EXPR_DEPTH;

/// A path through predicate nodes: `0` selects a unary child or left side,
/// `1` selects a right side. The addressed predicate variant determines arity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PredicatePath(pub Vec<u8>);

/// A path through numeric expression nodes: `0` is left and `1` is right.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExprPath(pub Vec<u8>);

/// Root of an existing numeric expression in an action or comparison guard.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ValueRoot {
    Action {
        transition: usize,
        index: usize,
    },
    GuardLeft {
        transition: usize,
        path: PredicatePath,
    },
    GuardRight {
        transition: usize,
        path: PredicatePath,
    },
}

impl PredicatePath {}

impl ExprPath {}

/// Borrow the expression at a root address after the shared projection bounds check.
pub fn inspect_value_root<'a>(
    graph: &'a BehaviorGraph,
    root: &ValueRoot,
) -> Result<&'a Expr, String> {
    preflight(graph)?;
    expression_at_root(graph, root)
}

/// Borrow an expression subtree at a root plus a binary expression path.
pub fn inspect_expression<'a>(
    graph: &'a BehaviorGraph,
    root: &ValueRoot,
    path: &ExprPath,
) -> Result<&'a Expr, String> {
    let expression = inspect_value_root(graph, root)?;
    expression_at_path(expression, path)
}

/// Borrow an expression after the caller has already projected this exact graph.
///
/// This skips the full-graph preflight for repeated UI lookups. Callers must
/// first call `field_projection::project` on the same, unchanged `graph` in
/// the current operation/frame. Address path and arity checks still run here.
pub(super) fn inspect_preflighted_expression<'a>(
    graph: &'a BehaviorGraph,
    root: &ValueRoot,
    path: &ExprPath,
) -> Result<&'a Expr, String> {
    let expression = expression_at_root(graph, root)?;
    expression_at_path(expression, path)
}

/// Borrow a predicate at a transition plus its predicate path.
pub fn inspect_predicate<'a>(
    graph: &'a BehaviorGraph,
    transition_index: usize,
    path: &PredicatePath,
) -> Result<&'a Predicate, String> {
    preflight(graph)?;
    predicate_at_transition(graph, transition_index, path)
}

/// Borrow a predicate after the caller has already projected this exact graph.
///
/// This skips the full-graph preflight for repeated UI lookups. Callers must
/// first call `field_projection::project` on the same, unchanged `graph` in
/// the current operation/frame. Address path and arity checks still run here.
pub(super) fn inspect_preflighted_predicate<'a>(
    graph: &'a BehaviorGraph,
    transition_index: usize,
    path: &PredicatePath,
) -> Result<&'a Predicate, String> {
    predicate_at_transition(graph, transition_index, path)
}

/// Replace one existing numeric expression subtree with a checked expression.
///
/// The incoming tree is bounded and checked for finite, in-range literals before
/// the graph or destination action/transition is cloned. Its inferred type must
/// match the expression subtree it replaces.
pub fn replace_numeric_expression(
    graph: &BehaviorGraph,
    fighter: &str,
    root: &ValueRoot,
    path: &ExprPath,
    replacement: Expr,
) -> Result<BehaviorGraph, String> {
    preflight(graph)?;
    bounded_input(fighter, live_model::MAX_FIGHTER_BYTES, "fighter name")?;
    if fighter.contains('\0') {
        return Err("fighter name must not contain a NUL byte".into());
    }
    let depth = expression_path_depth(root, path)?;
    preflight_expression(&replacement, depth)?;
    replace_numeric_expression_inner(graph, fighter, root, path, replacement)
}

fn replace_numeric_expression_inner(
    graph: &BehaviorGraph,
    fighter: &str,
    root: &ValueRoot,
    path: &ExprPath,
    replacement: Expr,
) -> Result<BehaviorGraph, String> {
    let current_root = expression_at_root(graph, root)?;
    let current = expression_at_path(current_root, path)?;
    let event_scopes = super::behavior::EventReadScopes::checked(graph)?;
    let transition_index = root_transition(root);
    let old_kind = expression_type(graph, &event_scopes, transition_index, current)?;
    let new_kind = expression_type(graph, &event_scopes, transition_index, &replacement)?;
    if !matches!(old_kind, VarType::Int | VarType::Float) || old_kind != new_kind {
        return Err(format!(
            "replacement type {new_kind:?} must match the addressed numeric expression type {old_kind:?}"
        ));
    }

    match root {
        ValueRoot::Action { transition, index } => {
            let action = graph
                .transitions
                .get(*transition)
                .and_then(|transition| transition.actions.get(*index))
                .ok_or_else(|| {
                    format!("action {index} in transition {transition} is out of range")
                })?;
            if reusable_timer_action_is_managed(graph, action) {
                return Err(
                    "reusable timer bookkeeping values are controlled by Duration and Repeat"
                        .into(),
                );
            }
            let mut updated = action.clone();
            let expression = match &mut updated {
                Action::Set { value, .. }
                | Action::Add { value, .. }
                | Action::Resource(crate::gimmicks::resource_commands::Command {
                    amount: value,
                    ..
                }) => value,
                _ => return Err("the addressed action does not contain a numeric value".into()),
            };
            replace_expression_at(expression, path, replacement)?;
            graph_edit::replace_action(graph, fighter, *transition, *index, updated)
        }
        ValueRoot::GuardLeft {
            transition,
            path: predicate_path,
        }
        | ValueRoot::GuardRight {
            transition,
            path: predicate_path,
        } => {
            reject_managed_timer_guard(graph, *transition)?;
            let mut updated = graph
                .transitions
                .get(*transition)
                .cloned()
                .ok_or_else(|| format!("transition index {transition} is out of range"))?;
            let comparison = predicate_at_path_mut(&mut updated.guard, predicate_path)?;
            let (left, right) = match comparison {
                Predicate::Compare { left, right, .. } => (left, right),
                _ => return Err("guard value root does not address a comparison".into()),
            };
            let target = if matches!(root, ValueRoot::GuardLeft { .. }) {
                left
            } else {
                right
            };
            replace_expression_at(target, path, replacement)?;
            graph_edit::replace_transition(graph, fighter, *transition, updated)
        }
    }
}

/// Replace a predicate subtree with a checked typed predicate.
///
/// Callers that wrap an existing condition should first inspect it and retain it
/// as a child of the new And/Or/Not tree. This operation never invents a default
/// truth value or discards the current subtree implicitly.
pub fn replace_predicate(
    graph: &BehaviorGraph,
    fighter: &str,
    transition_index: usize,
    path: &PredicatePath,
    replacement: Predicate,
) -> Result<BehaviorGraph, String> {
    preflight(graph)?;
    bounded_input(fighter, live_model::MAX_FIGHTER_BYTES, "fighter name")?;
    if fighter.contains('\0') {
        return Err("fighter name must not contain a NUL byte".into());
    }
    check_path(&path.0)?;
    preflight_predicate(&replacement, path.0.len() + 1)?;
    reject_managed_timer_guard(graph, transition_index)?;

    let mut transition = graph
        .transitions
        .get(transition_index)
        .cloned()
        .ok_or_else(|| format!("transition index {transition_index} is out of range"))?;
    *predicate_at_path_mut(&mut transition.guard, path)? = replacement;
    graph_edit::replace_transition(graph, fighter, transition_index, transition)
}

/// Infer the numeric type of an addressed expression for a typed field palette.
pub fn expression_kind(
    graph: &BehaviorGraph,
    root: &ValueRoot,
    path: &ExprPath,
) -> Result<VarType, String> {
    let expression = inspect_expression(graph, root, path)?;
    let event_scopes = super::behavior::EventReadScopes::checked(graph)?;
    let kind = expression_type(graph, &event_scopes, root_transition(root), expression)?;
    if matches!(kind, VarType::Int | VarType::Float) {
        Ok(kind)
    } else {
        Err(format!("addressed expression is {kind:?}, not numeric"))
    }
}

fn preflight(graph: &BehaviorGraph) -> Result<(), String> {
    // `project` performs an iterative, bounded walk before any edit clones a
    // transition/action or any inspector follows a nested path.
    field_projection::project(graph).map(|_| ())
}

fn bounded_input(value: &str, limit: usize, label: &str) -> Result<(), String> {
    if value.len() > limit {
        Err(format!("{label} exceeds the {limit}-byte limit"))
    } else {
        Ok(())
    }
}

fn expression_at_root<'a>(graph: &'a BehaviorGraph, root: &ValueRoot) -> Result<&'a Expr, String> {
    match root {
        ValueRoot::Action { transition, index } => graph
            .transitions
            .get(*transition)
            .and_then(|transition| transition.actions.get(*index))
            .and_then(action_expression)
            .ok_or_else(|| {
                format!("action {index} in transition {transition} has no numeric value")
            }),
        ValueRoot::GuardLeft { transition, path } => {
            let predicate = predicate_at_transition(graph, *transition, path)?;
            match predicate {
                Predicate::Compare { left, .. } => Ok(left),
                _ => Err("left guard value root does not address a comparison".into()),
            }
        }
        ValueRoot::GuardRight { transition, path } => {
            let predicate = predicate_at_transition(graph, *transition, path)?;
            match predicate {
                Predicate::Compare { right, .. } => Ok(right),
                _ => Err("right guard value root does not address a comparison".into()),
            }
        }
    }
}

fn predicate_at_transition<'a>(
    graph: &'a BehaviorGraph,
    transition_index: usize,
    path: &PredicatePath,
) -> Result<&'a Predicate, String> {
    let transition = graph
        .transitions
        .get(transition_index)
        .ok_or_else(|| format!("transition index {transition_index} is out of range"))?;
    predicate_at_path(&transition.guard, path)
}

fn action_expression(action: &Action) -> Option<&Expr> {
    match action {
        Action::Set { value, .. }
        | Action::Add { value, .. }
        | Action::Resource(crate::gimmicks::resource_commands::Command { amount: value, .. }) => {
            Some(value)
        }
        _ => None,
    }
}

fn expression_at_path<'a>(root: &'a Expr, path: &ExprPath) -> Result<&'a Expr, String> {
    check_path(&path.0)?;
    let mut current = root;
    for (depth, side) in path.0.iter().enumerate() {
        current = match (current, side) {
            (Expr::Add(left, _), 0) | (Expr::Sub(left, _), 0) | (Expr::Mul(left, _), 0) => left,
            (Expr::Add(_, right), 1) | (Expr::Sub(_, right), 1) | (Expr::Mul(_, right), 1) => right,
            (_, side) if *side > 1 => {
                return Err(format!(
                    "expression path side {side} is invalid at depth {depth}"
                ));
            }
            _ => return Err(format!("expression path leaves a scalar at depth {depth}")),
        };
    }
    Ok(current)
}

fn predicate_at_path<'a>(
    root: &'a Predicate,
    path: &PredicatePath,
) -> Result<&'a Predicate, String> {
    check_path(&path.0)?;
    let mut current = root;
    for (depth, side) in path.0.iter().enumerate() {
        current = match (current, side) {
            (Predicate::Not(inner), 0) => inner,
            (Predicate::And(left, _), 0) | (Predicate::Or(left, _), 0) => left,
            (Predicate::And(_, right), 1) | (Predicate::Or(_, right), 1) => right,
            (_, side) if *side > 1 => {
                return Err(format!(
                    "predicate path side {side} is invalid at depth {depth}"
                ));
            }
            _ => {
                return Err(format!(
                    "predicate path leaves a predicate leaf at depth {depth}"
                ));
            }
        };
    }
    Ok(current)
}

fn predicate_at_path_mut<'a>(
    root: &'a mut Predicate,
    path: &PredicatePath,
) -> Result<&'a mut Predicate, String> {
    check_path(&path.0)?;
    let mut current = root;
    for (depth, side) in path.0.iter().enumerate() {
        current = match (current, side) {
            (Predicate::Not(inner), 0) => inner,
            (Predicate::And(left, _), 0) | (Predicate::Or(left, _), 0) => left,
            (Predicate::And(_, right), 1) | (Predicate::Or(_, right), 1) => right,
            (_, side) if *side > 1 => {
                return Err(format!(
                    "predicate path side {side} is invalid at depth {depth}"
                ));
            }
            _ => {
                return Err(format!(
                    "predicate path leaves a predicate leaf at depth {depth}"
                ));
            }
        };
    }
    Ok(current)
}

fn replace_expression_at(
    root: &mut Expr,
    path: &ExprPath,
    replacement: Expr,
) -> Result<(), String> {
    check_path(&path.0)?;
    let mut current = root;
    for (depth, side) in path.0.iter().enumerate() {
        current = match (current, side) {
            (Expr::Add(left, _), 0) | (Expr::Sub(left, _), 0) | (Expr::Mul(left, _), 0) => left,
            (Expr::Add(_, right), 1) | (Expr::Sub(_, right), 1) | (Expr::Mul(_, right), 1) => right,
            (_, side) if *side > 1 => {
                return Err(format!(
                    "expression path side {side} is invalid at depth {depth}"
                ));
            }
            _ => return Err(format!("expression path leaves a scalar at depth {depth}")),
        };
    }
    *current = replacement;
    Ok(())
}

fn check_path(path: &[u8]) -> Result<(), String> {
    if path.len() > MAX_PATH_DEPTH {
        return Err(format!(
            "field value path exceeds depth limit {MAX_PATH_DEPTH}"
        ));
    }
    if path.iter().any(|side| *side > 1) {
        return Err("field value path sides must be 0 or 1".into());
    }
    Ok(())
}

fn expression_path_depth(root: &ValueRoot, path: &ExprPath) -> Result<usize, String> {
    check_path(&path.0)?;
    let base = match root {
        ValueRoot::Action { .. } => 1,
        ValueRoot::GuardLeft {
            path: predicate_path,
            ..
        }
        | ValueRoot::GuardRight {
            path: predicate_path,
            ..
        } => {
            check_path(&predicate_path.0)?;
            predicate_path.0.len().saturating_add(2)
        }
    };
    let depth = base.saturating_add(path.0.len());
    if depth > MAX_PATH_DEPTH {
        Err(format!(
            "expression path exceeds depth limit {MAX_PATH_DEPTH}"
        ))
    } else {
        Ok(depth)
    }
}

fn preflight_expression(expression: &Expr, root_depth: usize) -> Result<(), String> {
    let mut total = 0usize;
    preflight_expression_with_total(expression, root_depth, &mut total)
}

fn preflight_expression_with_total(
    expression: &Expr,
    root_depth: usize,
    total: &mut usize,
) -> Result<(), String> {
    let mut pending = vec![(expression, root_depth)];
    while let Some((expression, depth)) = pending.pop() {
        add_input_node(total, depth)?;
        match expression {
            Expr::StatusFrame
            | Expr::MotionFrame
            | Expr::StickX { .. }
            | Expr::StickY
            | Expr::RandomInt { .. }
            | Expr::RandomFloat { .. } => {}
            Expr::PendingDamage => {}
            Expr::Const(ConstValue::Float(value)) => {
                if !value.is_finite() || f64::from(*value).abs() > live_model::MAX_ABS_NUMBER {
                    return Err("incoming float is non-finite or outside the runtime range".into());
                }
            }
            Expr::Const(ConstValue::Int(value)) => {
                if i64::from(*value).abs() > live_model::MAX_ABS_NUMBER as i64 {
                    return Err("incoming integer is outside the runtime range".into());
                }
            }
            Expr::Const(ConstValue::Flag(_)) => {}
            Expr::NativeFloat(source) | Expr::NativeInt(source) => {
                if !source.reference_is_bounded() {
                    return Err(
                        "native input reference is empty, too long or contains a NUL byte".into(),
                    );
                }
            }
            Expr::Var(name) => {
                bounded_input(name, live_model::MAX_NAME_BYTES, "expression variable")?;
                if name.contains('\0') {
                    return Err("expression variable contains a NUL byte".into());
                }
            }
            Expr::TimerRemaining(name) => {
                bounded_input(name, live_model::MAX_NAME_BYTES, "timer variable")?;
                if name.trim().is_empty() || name.contains('\0') {
                    return Err("timer variable must be non-empty and NUL-free".into());
                }
            }
            Expr::EventField { event, field } => {
                for (name, label) in [(event, "event name"), (field, "event field name")] {
                    bounded_input(name, live_model::MAX_NAME_BYTES, label)?;
                    if name.trim().is_empty() || name.contains('\0') {
                        return Err(format!("{label} must be non-empty and NUL-free"));
                    }
                }
            }
            Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
                pending.push((right, depth + 1));
                pending.push((left, depth + 1));
            }
        }
    }
    Ok(())
}

fn preflight_predicate(predicate: &Predicate, root_depth: usize) -> Result<(), String> {
    let mut total = 0usize;
    let mut pending = vec![(predicate, root_depth)];
    while let Some((predicate, depth)) = pending.pop() {
        add_input_node(&mut total, depth)?;
        match predicate {
            Predicate::DamageIsProjectile
            | Predicate::InStatus { .. }
            | Predicate::InMotion { .. }
            | Predicate::SituationChanged
            | Predicate::MotionEnded
            | Predicate::CancelEnabled
            | Predicate::TermEnabled { .. }
            | Predicate::Situation { .. }
            | Predicate::Button { .. } => {}
            Predicate::Flag(name) | Predicate::Expired(name) => {
                bounded_input(name, live_model::MAX_NAME_BYTES, "predicate variable")?;
                if name.contains('\0') {
                    return Err("predicate variable contains a NUL byte".into());
                }
            }
            Predicate::EventFlag { event, field } => {
                for (name, label) in [(event, "event name"), (field, "event field name")] {
                    bounded_input(name, live_model::MAX_NAME_BYTES, label)?;
                    if name.trim().is_empty() || name.contains('\0') {
                        return Err(format!("{label} must be non-empty and NUL-free"));
                    }
                }
            }
            Predicate::NativeFlag(source) | Predicate::DamageStatusIs(source) => {
                if !source.reference_is_bounded() {
                    return Err(
                        "native input reference is empty, too long or contains a NUL byte".into(),
                    );
                }
            }
            Predicate::Compare { left, right, .. } => {
                preflight_expression_with_total(left, depth + 1, &mut total)?;
                preflight_expression_with_total(right, depth + 1, &mut total)?;
            }
            Predicate::Not(inner) => pending.push((inner, depth + 1)),
            Predicate::And(left, right) | Predicate::Or(left, right) => {
                pending.push((right, depth + 1));
                pending.push((left, depth + 1));
            }
        }
    }
    Ok(())
}

fn add_input_node(total: &mut usize, depth: usize) -> Result<(), String> {
    if depth > live_model::MAX_EXPR_DEPTH {
        return Err(format!(
            "incoming expression or predicate exceeds depth limit {}",
            live_model::MAX_EXPR_DEPTH
        ));
    }
    *total = total.saturating_add(1);
    if *total > live_model::MAX_EXPR_NODES {
        return Err(format!(
            "incoming expression or predicate exceeds node limit {}",
            live_model::MAX_EXPR_NODES
        ));
    }
    Ok(())
}

fn expression_type(
    graph: &BehaviorGraph,
    event_scopes: &super::behavior::EventReadScopes<'_>,
    transition: usize,
    expression: &Expr,
) -> Result<VarType, String> {
    match expression {
        Expr::StatusFrame => Ok(VarType::Int),
        Expr::MotionFrame | Expr::StickX { .. } | Expr::StickY | Expr::RandomFloat { .. } => {
            Ok(VarType::Float)
        }
        Expr::RandomInt { .. } => Ok(VarType::Int),
        Expr::PendingDamage => Ok(VarType::Float),
        Expr::Const(ConstValue::Float(_)) => Ok(VarType::Float),
        Expr::Const(ConstValue::Int(_)) => Ok(VarType::Int),
        Expr::Const(ConstValue::Flag(_)) => Ok(VarType::Flag),
        Expr::NativeFloat(_) => Ok(VarType::Float),
        Expr::NativeInt(_) => Ok(VarType::Int),
        Expr::Var(name) => graph
            .vars
            .iter()
            .find(|variable| variable.name == *name)
            .map(|variable| variable.kind)
            .ok_or_else(|| format!("expression variable {name:?} is not declared")),
        Expr::TimerRemaining(name) => {
            match graph.vars.iter().find(|variable| variable.name == *name) {
                Some(variable) if variable.kind == VarType::Timer => Ok(VarType::Int),
                Some(variable) => Err(format!(
                    "timer remaining requires a Timer variable, got {:?}",
                    variable.kind
                )),
                None => Err(format!("timer variable {name:?} is not declared")),
            }
        }
        Expr::EventField { event, field } => {
            let kind = event_scopes.field_kind(transition, event, field)?;
            if matches!(kind, VarType::Int | VarType::Float) {
                Ok(kind)
            } else {
                Err(format!("event field type {kind:?} is not numeric"))
            }
        }
        Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
            let left = expression_type(graph, event_scopes, transition, left)?;
            let right = expression_type(graph, event_scopes, transition, right)?;
            match (left, right) {
                (VarType::Int, VarType::Int) => Ok(VarType::Int),
                (VarType::Float, VarType::Float) => Ok(VarType::Float),
                _ => Err("expression operands do not have one matching numeric type".into()),
            }
        }
    }
}

fn root_transition(root: &ValueRoot) -> usize {
    match root {
        ValueRoot::Action { transition, .. }
        | ValueRoot::GuardLeft { transition, .. }
        | ValueRoot::GuardRight { transition, .. } => *transition,
    }
}

pub(super) fn reusable_timer_action_is_managed(graph: &BehaviorGraph, action: &Action) -> bool {
    super::controller::extract_timer(graph).is_ok()
        && action_target(action).is_some_and(is_managed_timer_variable)
}

fn is_managed_timer_variable(name: &str) -> bool {
    matches!(name, "remaining" | "ready" | "elapsed_events")
}

fn action_target(action: &Action) -> Option<&str> {
    match action {
        Action::Status(_)
        | Action::AddHeldOpponentDamage { .. }
        | Action::AddDamage { .. }
        | Action::CombatPolicy { .. }
        | Action::ThrowHeldOpponent { .. }
        | Action::ReleaseHeldOpponent
        | Action::CancelIncomingHit
        | Action::Movement(_)
        | Action::Resource(_)
        | Action::Item(_)
        | Action::MeshVisibility { .. }
        | Action::SetMotionRate { .. }
        | Action::ChooseHeroMenuSpell { .. }
        | Action::AddSpeed { .. }
        | Action::Object { .. }
        | Action::SetNative { .. }
        | Action::AddNative { .. }
        | Action::SetNativeFlag { .. } => None,
        Action::Set { var, .. }
        | Action::SetFlag { var, .. }
        | Action::Add { var, .. }
        | Action::Raise { var }
        | Action::Clear { var }
        | Action::Reset { var } => Some(var),
        Action::GenerateArticle { .. }
        | Action::RemoveArticle { .. }
        | Action::RemoveNewestArticle { .. }
        | Action::ArticleMovement { .. }
        | Action::RequestArticleStatus { .. }
        | Action::RequestFighterStatus { .. }
        | Action::RequestWeaponStatus { .. }
        | Action::EmitEvent { .. }
        | Action::EmitEventArguments { .. } => None,
        Action::EmitEventValues { .. } => None,
    }
}

/// Lifecycle rows of a reusable timer are edited as one unit by Duration/Repeat.
pub(super) fn is_managed_timer_transition(graph: &BehaviorGraph, transition: usize) -> bool {
    super::controller::extract_timer(graph).is_ok()
        && graph.emission_plan.bindings.iter().any(|binding| {
            matches!(binding.hook, HookName::OnTimeout | HookName::OnStockStart)
                && binding.transitions.contains(&transition)
        })
}

fn reject_managed_timer_guard(graph: &BehaviorGraph, transition: usize) -> Result<(), String> {
    if is_managed_timer_transition(graph, transition) {
        Err("reusable timer lifecycle guards are controlled by Duration and Repeat".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::{
        behavior::{CmpOp, StateDef, Transition, VarDef},
        codegen::{EmissionPlan, HookBinding, StatusTrigger},
        hooks::HookName,
    };

    fn nested_graph() -> BehaviorGraph {
        BehaviorGraph {
            custom_statuses: Default::default(),
            initial: "idle".into(),
            states: vec![StateDef {
                status: None,
                name: "idle".into(),
            }],
            vars: vec![
                VarDef {
                    keep_across_form_switch: false,
                    name: "whole".into(),
                    kind: VarType::Float,
                    initial: ConstValue::Float(0.0),
                },
                VarDef {
                    keep_across_form_switch: false,
                    name: "part".into(),
                    kind: VarType::Float,
                    initial: ConstValue::Float(0.0),
                },
                VarDef {
                    keep_across_form_switch: false,
                    name: "count".into(),
                    kind: VarType::Int,
                    initial: ConstValue::Int(0),
                },
                VarDef {
                    keep_across_form_switch: false,
                    name: "ticks".into(),
                    kind: VarType::Timer,
                    initial: ConstValue::Int(20),
                },
                VarDef {
                    keep_across_form_switch: false,
                    name: "enabled".into(),
                    kind: VarType::Flag,
                    initial: ConstValue::Flag(false),
                },
                VarDef {
                    keep_across_form_switch: false,
                    name: "alternate".into(),
                    kind: VarType::Flag,
                    initial: ConstValue::Flag(true),
                },
            ],
            transitions: vec![Transition {
                from: "idle".into(),
                to: "idle".into(),
                guard: Predicate::And(
                    Box::new(Predicate::Compare {
                        op: CmpOp::Gte,
                        left: Expr::Add(
                            Box::new(Expr::Var("whole".into())),
                            Box::new(Expr::Mul(
                                Box::new(Expr::Var("part".into())),
                                Box::new(Expr::Const(ConstValue::Float(2.0))),
                            )),
                        ),
                        right: Expr::Const(ConstValue::Float(4.0)),
                    }),
                    Box::new(Predicate::Or(
                        Box::new(Predicate::Flag("enabled".into())),
                        Box::new(Predicate::Not(Box::new(Predicate::Expired("ticks".into())))),
                    )),
                ),
                on_frame: true,
                actions: vec![
                    Action::Set {
                        var: "whole".into(),
                        value: Expr::Add(
                            Box::new(Expr::Var("whole".into())),
                            Box::new(Expr::Mul(
                                Box::new(Expr::Var("part".into())),
                                Box::new(Expr::Const(ConstValue::Float(2.0))),
                            )),
                        ),
                    },
                    Action::Add {
                        var: "count".into(),
                        value: Expr::Const(ConstValue::Int(1)),
                    },
                ],
            }],
            emission_plan: EmissionPlan {
                bindings: vec![HookBinding {
                    hook: HookName::OnStatusEnter,
                    article: None,
                    transitions: vec![0],
                }],
                status_triggers: vec![StatusTrigger {
                    hook: HookName::OnStatusEnter,
                    symbol: "FIGHTER_STATUS_KIND_SPECIAL_N".into(),
                    evidence: "retained review evidence".into(),
                    runtime_validated: true,
                    transitions: vec![0],
                }],
                ..EmissionPlan::default()
            },
        }
    }

    #[test]
    fn inspectors_address_nested_guard_and_action_expressions_without_cloning() {
        let graph = nested_graph();
        let root = ValueRoot::GuardLeft {
            transition: 0,
            path: PredicatePath(vec![0]),
        };
        assert!(matches!(
            inspect_expression(&graph, &root, &ExprPath(vec![1, 0])).unwrap(),
            Expr::Var(name) if name == "part"
        ));
        assert!(matches!(
            inspect_predicate(&graph, 0, &PredicatePath(vec![1, 1])).unwrap(),
            Predicate::Not(_)
        ));
        assert!(matches!(
            inspect_expression(
                &graph,
                &ValueRoot::Action {
                    transition: 0,
                    index: 0,
                },
                &ExprPath(vec![1, 1]),
            )
            .unwrap(),
            Expr::Const(ConstValue::Float(2.0))
        ));
    }

    #[test]
    fn typed_formula_and_predicate_replacements_preserve_existing_subtrees_and_plan() {
        let graph = nested_graph();
        let plan = graph.emission_plan.clone();
        let action_root = ValueRoot::Action {
            transition: 0,
            index: 0,
        };
        assert_eq!(
            expression_kind(&graph, &action_root, &ExprPath(vec![1])).unwrap(),
            VarType::Float
        );
        let formula = Expr::Sub(
            Box::new(Expr::Var("whole".into())),
            Box::new(Expr::Mul(
                Box::new(Expr::Var("part".into())),
                Box::new(Expr::Const(ConstValue::Float(3.0))),
            )),
        );
        let updated = replace_numeric_expression(
            &graph,
            "cloud",
            &action_root,
            &ExprPath(vec![1]),
            formula.clone(),
        )
        .unwrap();
        assert_eq!(
            inspect_expression(&updated, &action_root, &ExprPath(vec![1])).unwrap(),
            &formula
        );
        assert_eq!(updated.emission_plan, plan);

        let old_condition = inspect_predicate(&graph, 0, &PredicatePath(vec![1]))
            .unwrap()
            .clone();
        let wrapped = Predicate::And(
            Box::new(old_condition.clone()),
            Box::new(Predicate::Flag("alternate".into())),
        );
        let guarded =
            replace_predicate(&graph, "cloud", 0, &PredicatePath(vec![1]), wrapped).unwrap();
        assert_eq!(
            inspect_predicate(&guarded, 0, &PredicatePath(vec![1, 0])).unwrap(),
            &old_condition
        );
        assert_eq!(guarded.emission_plan, plan);
    }
}
