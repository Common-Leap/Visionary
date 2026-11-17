//! Bounded validation and variable remapping for legacy move behavior.

use std::collections::{BTreeMap, BTreeSet};

use crate::gimmicks::behavior::{Action, BehaviorGraph, Expr, Predicate};
use crate::gimmicks::live_model::{
    MAX_ACTIONS_PER_TRANSITION, MAX_EXPR_DEPTH, MAX_EXPR_NODES, MAX_NAME_BYTES, MAX_SYMBOL_BYTES,
    MAX_TRANSITIONS, MAX_VARIABLES,
};
use crate::gimmicks::native_inputs::CombatOp;

use super::model::{MoveGraph, RoutineArgumentValue, TriggerSource};

pub(super) const GENERATED_LOCAL_PREFIX: &str = "__mv_";
pub(super) const ROUTINE_INPUT_PREFIX: &str = "__ri_";
pub(super) const ROUTINE_OUTPUT_PREFIX: &str = "__ro_";

/// Stable, reserved Expr::Var spelling for a formal read in a routine body.
/// The port label remains the display/persistence identity.
pub(crate) fn routine_input_reference(input: &str) -> Result<String, String> {
    validate_variable_name(input)?;
    let hash = stable_hash([input.as_bytes()]);
    Ok(format!("{ROUTINE_INPUT_PREFIX}{hash:016x}"))
}

/// Stable, reserved writable reference for one mutable output in a routine body.
pub(crate) fn routine_output_reference(output: &str) -> Result<String, String> {
    validate_variable_name(output)?;
    let hash = stable_hash([output.as_bytes()]);
    Ok(format!("{ROUTINE_OUTPUT_PREFIX}{hash:016x}"))
}

/// Stable private storage slot for one nested callsite. The full caller path
/// distinguishes repeated uses of the same nested definition at different
/// locations and across independent trigger invocations.
pub(crate) fn routine_argument_variable_name_for_path(
    fighter: &str,
    move_name: &str,
    trigger: &str,
    call_path: &[usize],
    routine: &str,
    input: &str,
) -> Result<String, String> {
    for (name, label) in [
        (fighter, "fighter name"),
        (move_name, "move name"),
        (trigger, "trigger name"),
        (routine, "routine name"),
        (input, "routine input name"),
    ] {
        bounded_name(name, label)?;
        if name.contains('\0') {
            return Err(format!("{label} must not contain NUL bytes"));
        }
    }
    if call_path.is_empty() || call_path.len() > super::expand::MAX_ROUTINE_DEPTH {
        return Err("routine call path is empty or exceeds the nesting limit".into());
    }
    let mut path_bytes = Vec::with_capacity(call_path.len() * std::mem::size_of::<u64>());
    for index in call_path {
        path_bytes.extend_from_slice(&(*index as u64).to_le_bytes());
    }
    let hash = stable_hash([
        fighter.as_bytes(),
        move_name.as_bytes(),
        trigger.as_bytes(),
        path_bytes.as_slice(),
        routine.as_bytes(),
        input.as_bytes(),
    ]);
    Ok(format!("{GENERATED_LOCAL_PREFIX}arg_{hash:016x}"))
}

/// Stable private storage slot for one routine output at a nested callsite.
pub(crate) fn routine_output_variable_name_for_path(
    fighter: &str,
    move_name: &str,
    trigger: &str,
    call_path: &[usize],
    routine: &str,
    output: &str,
) -> Result<String, String> {
    for (name, label) in [
        (fighter, "fighter name"),
        (move_name, "move name"),
        (trigger, "trigger name"),
        (routine, "routine name"),
        (output, "routine output name"),
    ] {
        bounded_name(name, label)?;
        if name.contains('\0') {
            return Err(format!("{label} must not contain NUL bytes"));
        }
    }
    if call_path.is_empty() || call_path.len() > super::expand::MAX_ROUTINE_DEPTH {
        return Err("routine call path is empty or exceeds the nesting limit".into());
    }
    let mut path_bytes = Vec::with_capacity(call_path.len() * std::mem::size_of::<u64>());
    for index in call_path {
        path_bytes.extend_from_slice(&(*index as u64).to_le_bytes());
    }
    let hash = stable_hash([
        fighter.as_bytes(),
        move_name.as_bytes(),
        trigger.as_bytes(),
        path_bytes.as_slice(),
        routine.as_bytes(),
        output.as_bytes(),
    ]);
    Ok(format!("{GENERATED_LOCAL_PREFIX}out_{hash:016x}"))
}

fn stable_hash<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for part in parts {
        for byte in part.iter().copied().chain([0xff]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

/// Stable internal name for a move-local variable. This is intentionally an
/// opaque, bounded identifier; UI labels should retain the authored move/name.
pub(crate) fn runtime_variable_name(move_name: &str, var_name: &str) -> Result<String, String> {
    bounded_name(move_name, "move name")?;
    validate_variable_name(var_name)?;
    if move_name.contains('\0') || var_name.starts_with(GENERATED_LOCAL_PREFIX) {
        return Err("invalid move-local variable namespace input".into());
    }
    let mut hash = 0x811c9dc5u32;
    for byte in move_name.bytes().chain([0xff]).chain(var_name.bytes()) {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    let prefix = format!("{GENERATED_LOCAL_PREFIX}{hash:08x}_");
    let budget = MAX_NAME_BYTES.saturating_sub(prefix.len());
    let mut suffix = String::new();
    for ch in var_name.chars() {
        if suffix.len().saturating_add(ch.len_utf8()) > budget {
            break;
        }
        suffix.push(ch);
    }
    Ok(format!("{prefix}{suffix}"))
}

/// Validate bounds and scope addresses before cloning or rewriting trigger trees.
pub(super) fn preflight_scopes(graph: &MoveGraph) -> Result<(), String> {
    if graph.variables.len() > MAX_VARIABLES {
        return Err(format!(
            "move graph exceeds {MAX_VARIABLES} variable declarations"
        ));
    }
    if graph.triggers.len() > MAX_TRANSITIONS {
        return Err(format!(
            "move graph exceeds {MAX_TRANSITIONS} trigger declarations"
        ));
    }

    let mut declared = BTreeSet::new();
    for variable in &graph.variables {
        validate_variable_name(&variable.name)?;
        if variable.name.starts_with(GENERATED_LOCAL_PREFIX)
            || variable.name.starts_with(ROUTINE_INPUT_PREFIX)
            || variable.name.starts_with(ROUTINE_OUTPUT_PREFIX)
        {
            return Err(format!(
                "variable name {:?} uses a reserved generated namespace",
                variable.name
            ));
        }
        if !declared.insert(variable.name.as_str()) {
            return Err(format!(
                "variable {:?} is declared more than once",
                variable.name
            ));
        }
    }
    for name in graph.variable_scopes.keys() {
        validate_variable_name(name)?;
        if !declared.contains(name.as_str()) {
            return Err(format!(
                "scope metadata refers to undeclared variable {name:?}"
            ));
        }
    }

    let mut expression_nodes = 0usize;
    let mut trigger_names = BTreeSet::new();
    for trigger in &graph.triggers {
        if trigger.name.trim().is_empty()
            || trigger.name.len() > MAX_NAME_BYTES
            || trigger.name.contains('\0')
        {
            return Err(
                "trigger names must be non-empty, NUL-free, and within the name limit".into(),
            );
        }
        if !trigger_names.insert(trigger.name.as_str()) {
            return Err(format!(
                "trigger name {:?} is declared more than once",
                trigger.name
            ));
        }
        if trigger.actions.len() > MAX_ACTIONS_PER_TRANSITION {
            return Err(format!(
                "trigger {:?} exceeds {MAX_ACTIONS_PER_TRANSITION} actions",
                trigger.name
            ));
        }
        match &trigger.source {
            TriggerSource::TimerExpired { timer } => validate_variable_name(timer)?,
            TriggerSource::Event { name, fields } => {
                if name.trim().is_empty() || name.len() > 48 || name.contains('\0') {
                    return Err(
                        "event names must be non-empty, NUL-free, and at most 48 bytes".into(),
                    );
                }
                crate::gimmicks::event_payload::validate_schema(&trigger.name, name, fields)
                    .map_err(|errors| {
                        errors
                            .into_iter()
                            .map(|error| error.to_string())
                            .collect::<Vec<_>>()
                            .join("; ")
                    })?;
            }
            _ => {}
        }
        if let Some(guard) = &trigger.guard {
            preflight_predicate(guard, &mut expression_nodes)?;
        }
        for action in &trigger.actions {
            preflight_action(action, &mut expression_nodes)?;
        }
    }

    if graph.routines.len() > MAX_TRANSITIONS {
        return Err(format!(
            "move graph exceeds {MAX_TRANSITIONS} routine definitions"
        ));
    }
    let aggregate_limit = MAX_ACTIONS_PER_TRANSITION.saturating_mul(MAX_TRANSITIONS);
    let mut aggregate_actions = 0usize;
    for (name, actions) in &graph.routines {
        validate_routine_name(name)?;
        if actions.len() > MAX_ACTIONS_PER_TRANSITION {
            return Err(format!(
                "routine {name:?} exceeds {MAX_ACTIONS_PER_TRANSITION} actions"
            ));
        }
        aggregate_actions = aggregate_actions.saturating_add(actions.len());
        if aggregate_actions > aggregate_limit {
            return Err(format!(
                "move graph routine bodies exceed aggregate action limit {aggregate_limit}"
            ));
        }
        for action in actions {
            preflight_action(action, &mut expression_nodes)?;
        }
    }

    if graph.routine_inputs.len() > graph.routines.len() {
        return Err("routine input signatures contain an unknown definition".into());
    }
    if graph.routine_outputs.len() > graph.routines.len() {
        return Err("routine output signatures contain an unknown definition".into());
    }
    for (routine, outputs) in &graph.routine_outputs {
        validate_routine_name(routine)?;
        if !graph.routines.contains_key(routine) {
            return Err(format!(
                "output signature references unknown routine {routine:?}"
            ));
        }
        let inputs = graph
            .routine_inputs
            .get(routine)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if outputs.len().saturating_add(inputs.len())
            > crate::gimmicks::live_model::MAX_EVENT_FIELDS
        {
            return Err(format!(
                "routine {routine:?} exceeds the maximum of {} combined inputs and outputs",
                crate::gimmicks::live_model::MAX_EVENT_FIELDS
            ));
        }
        let mut names: BTreeSet<&str> = inputs.iter().map(|input| input.name.as_str()).collect();
        let mut references = BTreeSet::new();
        for output in outputs {
            validate_routine_name(&output.name)?;
            if !names.insert(output.name.as_str()) {
                return Err(format!(
                    "routine {routine:?} repeats or collides with an input named {:?}",
                    output.name
                ));
            }
            if declared.contains(output.name.as_str()) {
                return Err(format!(
                    "routine output {:?} collides with a declared variable",
                    output.name
                ));
            }
            if !matches!(
                output.kind,
                crate::gimmicks::behavior::VarType::Float
                    | crate::gimmicks::behavior::VarType::Int
                    | crate::gimmicks::behavior::VarType::Flag
            ) {
                return Err(format!(
                    "routine output {:?} must be Float, Int, or Flag (Timer identities cannot be returned)",
                    output.name
                ));
            }
            let reference = routine_output_reference(&output.name)?;
            if !references.insert(reference) {
                return Err(format!(
                    "routine {routine:?} has colliding output reference identifiers"
                ));
            }
        }
    }
    for (routine, inputs) in &graph.routine_inputs {
        validate_routine_name(routine)?;
        if !graph.routines.contains_key(routine) {
            return Err(format!(
                "input signature references unknown routine {routine:?}"
            ));
        }
        if inputs.len() > crate::gimmicks::live_model::MAX_EVENT_FIELDS {
            return Err(format!(
                "routine {routine:?} exceeds the maximum of {} inputs",
                crate::gimmicks::live_model::MAX_EVENT_FIELDS
            ));
        }
        let mut names = BTreeSet::new();
        let mut references = BTreeSet::new();
        for input in inputs {
            validate_routine_name(&input.name)?;
            if !names.insert(input.name.as_str()) {
                return Err(format!(
                    "routine {routine:?} repeats input {:?}",
                    input.name
                ));
            }
            if declared.contains(input.name.as_str()) {
                return Err(format!(
                    "routine {routine:?} input {:?} collides with a declared variable",
                    input.name
                ));
            }
            if !matches!(
                input.kind,
                crate::gimmicks::behavior::VarType::Float
                    | crate::gimmicks::behavior::VarType::Int
                    | crate::gimmicks::behavior::VarType::Flag
            ) {
                return Err(format!(
                    "routine {routine:?} input {:?} must be Float, Int, or Flag (Timer identities cannot be captured)",
                    input.name
                ));
            }
            let reference = routine_input_reference(&input.name)?;
            if !references.insert(reference) {
                return Err(format!(
                    "routine {routine:?} has colliding input reference identifiers"
                ));
            }
            expression_nodes = expression_nodes.saturating_add(1);
            if expression_nodes > MAX_EXPR_NODES {
                return Err(format!(
                    "move trigger expressions exceed node limit {MAX_EXPR_NODES}"
                ));
            }
        }
    }

    for (trigger_name, calls) in &graph.routine_calls {
        if !trigger_names.contains(trigger_name.as_str()) {
            return Err(format!(
                "routine calls refer to missing trigger {trigger_name:?}"
            ));
        }
        if calls.len() > MAX_ACTIONS_PER_TRANSITION {
            return Err(format!(
                "trigger {trigger_name:?} exceeds the routine-call limit"
            ));
        }
        let trigger = graph
            .triggers
            .iter()
            .find(|trigger| trigger.name == *trigger_name)
            .unwrap();
        for call in calls {
            validate_routine_name(&call.routine)?;
            if call.before > trigger.actions.len() {
                return Err(format!(
                    "routine call anchor {} exceeds trigger {trigger_name:?} action count {}",
                    call.before,
                    trigger.actions.len()
                ));
            }
            if call.arguments.len().saturating_add(call.outputs.len())
                > crate::gimmicks::live_model::MAX_EVENT_FIELDS
            {
                return Err(format!(
                    "routine call to {:?} exceeds the maximum of {} combined inputs and outputs",
                    call.routine,
                    crate::gimmicks::live_model::MAX_EVENT_FIELDS
                ));
            }
            let mut argument_names = BTreeSet::new();
            for argument in &call.arguments {
                validate_routine_name(&argument.input)?;
                if !argument_names.insert(argument.input.as_str()) {
                    return Err(format!(
                        "routine call to {:?} repeats input {:?}",
                        call.routine, argument.input
                    ));
                }
                match &argument.value {
                    RoutineArgumentValue::Numeric(expression) => {
                        preflight_expression(expression, &mut expression_nodes)?;
                    }
                    RoutineArgumentValue::Flag(predicate) => {
                        preflight_predicate(predicate, &mut expression_nodes)?;
                    }
                }
            }
            preflight_output_bindings(call)?;
        }
    }
    if graph.routine_calls_in_routines.len() > graph.routines.len() {
        return Err("nested routine calls contain an unknown routine definition".into());
    }
    for (routine_name, calls) in &graph.routine_calls_in_routines {
        validate_routine_name(routine_name)?;
        let body = graph
            .routines
            .get(routine_name)
            .ok_or_else(|| format!("nested calls refer to unknown routine {routine_name:?}"))?;
        if calls.len() > MAX_ACTIONS_PER_TRANSITION {
            return Err(format!(
                "routine {routine_name:?} exceeds the routine-call limit"
            ));
        }
        for call in calls {
            validate_routine_name(&call.routine)?;
            if call.before > body.len() {
                return Err(format!(
                    "nested routine call anchor {} exceeds routine {routine_name:?} action count {}",
                    call.before,
                    body.len()
                ));
            }
            if call.arguments.len().saturating_add(call.outputs.len())
                > crate::gimmicks::live_model::MAX_EVENT_FIELDS
            {
                return Err(format!(
                    "nested routine call to {:?} exceeds the maximum of {} combined inputs and outputs",
                    call.routine,
                    crate::gimmicks::live_model::MAX_EVENT_FIELDS
                ));
            }
            let mut argument_names = BTreeSet::new();
            for argument in &call.arguments {
                validate_routine_name(&argument.input)?;
                if !argument_names.insert(argument.input.as_str()) {
                    return Err(format!(
                        "nested routine call to {:?} repeats input {:?}",
                        call.routine, argument.input
                    ));
                }
                match &argument.value {
                    RoutineArgumentValue::Numeric(expression) => {
                        preflight_expression(expression, &mut expression_nodes)?;
                    }
                    RoutineArgumentValue::Flag(predicate) => {
                        preflight_predicate(predicate, &mut expression_nodes)?;
                    }
                }
            }
            preflight_output_bindings(call)?;
        }
    }
    Ok(())
}

fn preflight_output_bindings(call: &super::model::RoutineCall) -> Result<(), String> {
    let mut outputs = BTreeSet::new();
    let mut destinations = BTreeSet::new();
    for binding in &call.outputs {
        validate_routine_name(&binding.output)?;
        validate_variable_name(&binding.destination)?;
        if !outputs.insert(binding.output.as_str()) {
            return Err(format!(
                "routine call to {:?} repeats output binding {:?}",
                call.routine, binding.output
            ));
        }
        if !destinations.insert(binding.destination.as_str()) {
            return Err(format!(
                "routine call to {:?} binds multiple outputs to destination {:?}",
                call.routine, binding.destination
            ));
        }
    }
    Ok(())
}

fn preflight_action(action: &Action, expression_nodes: &mut usize) -> Result<(), String> {
    match action {
        Action::Status(c) => {
            for e in c.expressions() {
                preflight_expression(e, expression_nodes)?;
            }
        }
        Action::Object { op, .. } => {
            for e in op.expressions() {
                preflight_expression(e, expression_nodes)?;
            }
        }
        Action::AddSpeed { x, y } => {
            preflight_expression(x, expression_nodes)?;
            preflight_expression(y, expression_nodes)?;
        }
        Action::SetNative { value, .. } | Action::AddNative { value, .. } => {
            preflight_expression(value, expression_nodes)?
        }
        Action::SetNativeFlag { value, .. } => preflight_predicate(value, expression_nodes)?,
        Action::SetMotionRate { rate: amount }
        | Action::AddHeldOpponentDamage { amount }
        | Action::AddDamage { amount }
        | Action::CombatPolicy {
            op:
                CombatOp::DamageTaken(amount)
                | CombatOp::KnockbackTaken(amount)
                | CombatOp::DamageDealt(amount)
                | CombatOp::KnockbackDealt(amount)
                | CombatOp::Stat(_, amount)
                | CombatOp::Armor(amount),
            ..
        }
        | Action::Resource(crate::gimmicks::resource_commands::Command { amount, .. }) => {
            preflight_expression(amount, expression_nodes)?
        }
        Action::CombatPolicy { .. }
        | Action::ThrowHeldOpponent { .. }
        | Action::ReleaseHeldOpponent
        | Action::CancelIncomingHit
        | Action::Movement(_)
        | Action::Item(_)
        | Action::MeshVisibility { .. }
        | Action::ChooseHeroMenuSpell { .. } => {}
        Action::Set { var, value } | Action::Add { var, value } => {
            validate_variable_name(var)?;
            preflight_expression(value, expression_nodes)?;
        }
        Action::SetFlag { var, value } => {
            validate_variable_name(var)?;
            preflight_predicate(value, expression_nodes)?;
        }
        Action::Raise { var } | Action::Clear { var } | Action::Reset { var } => {
            validate_variable_name(var)?;
        }
        Action::EmitEvent { name } => validate_variable_name(name)?,
        Action::EmitEventArguments { name, arguments } => {
            validate_variable_name(name)?;
            crate::gimmicks::event_payload::validate_argument_shape(
                "move trigger",
                name,
                arguments,
            )
            .map_err(|errors| {
                errors
                    .into_iter()
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            })?;
        }
        Action::EmitEventValues { name, arguments } => {
            validate_variable_name(name)?;
            crate::gimmicks::event_payload::validate_value_argument_shape(
                "move trigger",
                name,
                arguments,
            )
            .map_err(|errors| {
                errors
                    .into_iter()
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            })?;
            for argument in arguments {
                match &argument.value {
                    crate::gimmicks::event_payload::EventValue::Constant(_) => {}
                    crate::gimmicks::event_payload::EventValue::Numeric(expr) => {
                        preflight_expression(expr, expression_nodes)?;
                    }
                    crate::gimmicks::event_payload::EventValue::Flag(predicate) => {
                        preflight_predicate(predicate, expression_nodes)?;
                    }
                }
            }
        }
        Action::GenerateArticle { symbol }
        | Action::RemoveArticle { symbol }
        | Action::RemoveNewestArticle { symbol }
        | Action::ArticleMovement { symbol, .. } => {
            bounded_symbol(symbol, "article symbol")?;
        }
        Action::RequestArticleStatus { symbol, status } => {
            bounded_symbol(symbol, "article symbol")?;
            bounded_symbol(status, "article status")?;
        }
        Action::RequestFighterStatus { symbol } | Action::RequestWeaponStatus { symbol, .. } => {
            bounded_symbol(symbol, "weapon status symbol")?;
        }
    }
    Ok(())
}

fn validate_routine_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.len() > MAX_NAME_BYTES || name.contains('\0') {
        Err(format!(
            "routine names must be non-empty, NUL-free, and within {MAX_NAME_BYTES} bytes"
        ))
    } else {
        Ok(())
    }
}

fn preflight_predicate(predicate: &Predicate, total: &mut usize) -> Result<(), String> {
    let mut pending = vec![(predicate, 1usize)];
    while let Some((predicate, depth)) = pending.pop() {
        add_bounded_node(total, depth)?;
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
            Predicate::Flag(name) | Predicate::Expired(name) => validate_variable_name(name)?,
            Predicate::EventFlag { event, field } => {
                validate_variable_name(event)?;
                validate_variable_name(field)?;
            }
            Predicate::NativeFlag(source) | Predicate::DamageStatusIs(source) => {
                validate_native_input_reference(source.symbol())?
            }
            Predicate::Compare { left, right, .. } => {
                preflight_expression_at(left, depth + 1, total)?;
                preflight_expression_at(right, depth + 1, total)?;
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

fn preflight_expression(expression: &Expr, total: &mut usize) -> Result<(), String> {
    preflight_expression_at(expression, 1, total)
}

fn preflight_expression_at(
    expression: &Expr,
    root_depth: usize,
    total: &mut usize,
) -> Result<(), String> {
    let mut pending = vec![(expression, root_depth)];
    while let Some((expression, depth)) = pending.pop() {
        add_bounded_node(total, depth)?;
        match expression {
            Expr::StatusFrame
            | Expr::MotionFrame
            | Expr::StickX { .. }
            | Expr::StickY
            | Expr::RandomInt { .. }
            | Expr::RandomFloat { .. } => {}
            Expr::PendingDamage => {}
            Expr::Var(name) => validate_variable_name(name)?,
            Expr::TimerRemaining(name) => validate_variable_name(name)?,
            Expr::EventField { event, field } => {
                validate_variable_name(event)?;
                validate_variable_name(field)?;
            }
            Expr::NativeFloat(source) | Expr::NativeInt(source) => {
                validate_native_input_reference(source.symbol())?
            }
            Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
                pending.push((right, depth + 1));
                pending.push((left, depth + 1));
            }
            Expr::Const(_) => {}
        }
    }
    Ok(())
}

fn add_bounded_node(total: &mut usize, depth: usize) -> Result<(), String> {
    if depth > MAX_EXPR_DEPTH {
        return Err(format!(
            "move trigger expression exceeds depth limit {MAX_EXPR_DEPTH}"
        ));
    }
    *total = total.saturating_add(1);
    if *total > MAX_EXPR_NODES {
        return Err(format!(
            "move trigger expressions exceed node limit {MAX_EXPR_NODES}"
        ));
    }
    Ok(())
}

fn validate_variable_name(name: &str) -> Result<(), String> {
    bounded_name(name, "variable/event name")?;
    if name.contains('\0') {
        return Err("variable/event names must not contain NUL bytes".into());
    }
    Ok(())
}

fn bounded_name(name: &str, label: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        Err(format!(
            "{label} must be non-empty and at most {MAX_NAME_BYTES} bytes"
        ))
    } else {
        Ok(())
    }
}

fn bounded_symbol(name: &str, label: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > MAX_SYMBOL_BYTES || name.contains('\0') {
        Err(format!(
            "{label} must be non-empty, NUL-free, and at most {MAX_SYMBOL_BYTES} bytes"
        ))
    } else {
        Ok(())
    }
}

fn validate_native_input_reference(symbol: &str) -> Result<(), String> {
    if symbol.is_empty()
        || symbol.len() > crate::gimmicks::native_inputs::NativeInput::MAX_REFERENCE_BYTES
        || symbol.contains('\0')
    {
        return Err(
            "native input reference must be non-empty, NUL-free, and within its byte limit".into(),
        );
    }
    Ok(())
}

/// Rewrite only typed variable references in one already-bounded trigger.
pub(super) fn remap_trigger_variables(
    trigger: &mut super::model::CustomTrigger,
    mapping: &BTreeMap<String, String>,
) {
    if let TriggerSource::TimerExpired { timer } = &mut trigger.source {
        remap_name(timer, mapping);
    }
    if let Some(guard) = &mut trigger.guard {
        remap_predicate(guard, mapping);
    }
    for action in &mut trigger.actions {
        remap_action(action, mapping);
    }
}

fn remap_action(action: &mut Action, mapping: &BTreeMap<String, String>) {
    match action {
        Action::Status(c) => {
            for e in c.expressions_mut() {
                remap_expr(e, mapping);
            }
        }
        Action::Object { op, .. } => {
            for e in op.expressions_mut() {
                remap_expr(e, mapping);
            }
        }
        Action::AddSpeed { x, y } => {
            remap_expr(x, mapping);
            remap_expr(y, mapping);
        }
        Action::SetNative { value, .. } | Action::AddNative { value, .. } => {
            remap_expr(value, mapping)
        }
        Action::SetNativeFlag { value, .. } => remap_predicate(value, mapping),
        Action::SetMotionRate { rate: amount }
        | Action::AddHeldOpponentDamage { amount }
        | Action::AddDamage { amount }
        | Action::CombatPolicy {
            op:
                CombatOp::DamageTaken(amount)
                | CombatOp::KnockbackTaken(amount)
                | CombatOp::DamageDealt(amount)
                | CombatOp::KnockbackDealt(amount)
                | CombatOp::Stat(_, amount)
                | CombatOp::Armor(amount),
            ..
        }
        | Action::Resource(crate::gimmicks::resource_commands::Command { amount, .. }) => {
            remap_expr(amount, mapping)
        }
        Action::CombatPolicy { .. }
        | Action::ThrowHeldOpponent { .. }
        | Action::ReleaseHeldOpponent
        | Action::CancelIncomingHit
        | Action::Movement(_)
        | Action::Item(_)
        | Action::MeshVisibility { .. }
        | Action::ChooseHeroMenuSpell { .. } => {}
        Action::Set { var, value } | Action::Add { var, value } => {
            remap_name(var, mapping);
            remap_expr(value, mapping);
        }
        Action::SetFlag { var, value } => {
            remap_name(var, mapping);
            remap_predicate(value, mapping);
        }
        Action::Raise { var } | Action::Clear { var } | Action::Reset { var } => {
            remap_name(var, mapping)
        }
        Action::GenerateArticle { .. }
        | Action::RemoveArticle { .. }
        | Action::RemoveNewestArticle { .. }
        | Action::ArticleMovement { .. }
        | Action::RequestArticleStatus { .. }
        | Action::RequestFighterStatus { .. }
        | Action::RequestWeaponStatus { .. }
        | Action::EmitEvent { .. }
        | Action::EmitEventArguments { .. } => {}
        Action::EmitEventValues { arguments, .. } => {
            for argument in arguments {
                match &mut argument.value {
                    crate::gimmicks::event_payload::EventValue::Constant(_) => {}
                    crate::gimmicks::event_payload::EventValue::Numeric(expr) => {
                        remap_expr(expr, mapping);
                    }
                    crate::gimmicks::event_payload::EventValue::Flag(predicate) => {
                        remap_predicate(predicate, mapping);
                    }
                }
            }
        }
    }
}

fn remap_expr(expression: &mut Expr, mapping: &BTreeMap<String, String>) {
    match expression {
        Expr::StatusFrame
        | Expr::MotionFrame
        | Expr::StickX { .. }
        | Expr::StickY
        | Expr::RandomInt { .. }
        | Expr::RandomFloat { .. } => {}
        Expr::PendingDamage => {}
        Expr::Var(name) => remap_name(name, mapping),
        Expr::TimerRemaining(name) => remap_name(name, mapping),
        Expr::NativeFloat(_) | Expr::NativeInt(_) | Expr::EventField { .. } => {}
        Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
            remap_expr(left, mapping);
            remap_expr(right, mapping);
        }
        Expr::Const(_) => {}
    }
}

fn remap_predicate(predicate: &mut Predicate, mapping: &BTreeMap<String, String>) {
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
        Predicate::Flag(name) | Predicate::Expired(name) => remap_name(name, mapping),
        Predicate::NativeFlag(_) | Predicate::DamageStatusIs(_) | Predicate::EventFlag { .. } => {}
        Predicate::Compare { left, right, .. } => {
            remap_expr(left, mapping);
            remap_expr(right, mapping);
        }
        Predicate::Not(inner) => remap_predicate(inner, mapping),
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            remap_predicate(left, mapping);
            remap_predicate(right, mapping);
        }
    }
}

fn remap_name(name: &mut String, mapping: &BTreeMap<String, String>) {
    if let Some(mapped) = mapping.get(name) {
        name.clone_from(mapped);
    }
}

pub(crate) fn routine_argument_variable_name(
    fighter: &str,
    move_name: &str,
    trigger: &str,
    call_index: usize,
    routine: &str,
    input: &str,
) -> Result<String, String> {
    routine_argument_variable_name_for_path(
        fighter,
        move_name,
        trigger,
        &[call_index],
        routine,
        input,
    )
}

pub fn behavior_referenced_variables(graph: &BehaviorGraph) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for transition in &graph.transitions {
        collect_predicate(&transition.guard, &mut names);
        for action in &transition.actions {
            collect_action(action, &mut names);
        }
    }
    names
}

fn collect_action(action: &Action, names: &mut BTreeSet<String>) {
    match action {
        Action::Status(c) => {
            for e in c.expressions() {
                collect_expr(e, names);
            }
        }
        Action::Object { op, .. } => {
            for e in op.expressions() {
                collect_expr(e, names);
            }
        }
        Action::AddSpeed { x, y } => {
            collect_expr(x, names);
            collect_expr(y, names);
        }
        Action::SetNative { value, .. } | Action::AddNative { value, .. } => {
            collect_expr(value, names)
        }
        Action::SetNativeFlag { value, .. } => collect_predicate(value, names),
        Action::SetMotionRate { rate: amount }
        | Action::AddHeldOpponentDamage { amount }
        | Action::AddDamage { amount }
        | Action::CombatPolicy {
            op:
                CombatOp::DamageTaken(amount)
                | CombatOp::KnockbackTaken(amount)
                | CombatOp::DamageDealt(amount)
                | CombatOp::KnockbackDealt(amount)
                | CombatOp::Stat(_, amount)
                | CombatOp::Armor(amount),
            ..
        }
        | Action::Resource(crate::gimmicks::resource_commands::Command { amount, .. }) => {
            collect_expr(amount, names)
        }
        Action::CombatPolicy { .. }
        | Action::ThrowHeldOpponent { .. }
        | Action::ReleaseHeldOpponent
        | Action::CancelIncomingHit
        | Action::Movement(_)
        | Action::Item(_)
        | Action::MeshVisibility { .. }
        | Action::ChooseHeroMenuSpell { .. } => {}
        Action::Set { var, value } | Action::Add { var, value } => {
            names.insert(var.clone());
            collect_expr(value, names);
        }
        Action::SetFlag { var, value } => {
            names.insert(var.clone());
            collect_predicate(value, names);
        }
        Action::Raise { var } | Action::Clear { var } | Action::Reset { var } => {
            names.insert(var.clone());
        }
        Action::GenerateArticle { .. }
        | Action::RemoveArticle { .. }
        | Action::RemoveNewestArticle { .. }
        | Action::ArticleMovement { .. }
        | Action::RequestArticleStatus { .. }
        | Action::RequestFighterStatus { .. }
        | Action::RequestWeaponStatus { .. }
        | Action::EmitEvent { .. }
        | Action::EmitEventArguments { .. } => {}
        Action::EmitEventValues { arguments, .. } => {
            for argument in arguments {
                match &argument.value {
                    crate::gimmicks::event_payload::EventValue::Constant(_) => {}
                    crate::gimmicks::event_payload::EventValue::Numeric(expr) => {
                        collect_expr(expr, names);
                    }
                    crate::gimmicks::event_payload::EventValue::Flag(predicate) => {
                        collect_predicate(predicate, names);
                    }
                }
            }
        }
    }
}

fn collect_expr(expr: &Expr, names: &mut BTreeSet<String>) {
    match expr {
        Expr::StatusFrame
        | Expr::MotionFrame
        | Expr::StickX { .. }
        | Expr::StickY
        | Expr::RandomInt { .. }
        | Expr::RandomFloat { .. } => {}
        Expr::PendingDamage => {}
        Expr::Var(name) => {
            if !name.starts_with(ROUTINE_INPUT_PREFIX) {
                names.insert(name.clone());
            }
        }
        Expr::TimerRemaining(name) => {
            names.insert(name.clone());
        }
        Expr::NativeFloat(_) | Expr::NativeInt(_) | Expr::EventField { .. } => {}
        Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
            collect_expr(left, names);
            collect_expr(right, names);
        }
        Expr::Const(_) => {}
    }
}

fn collect_predicate(predicate: &Predicate, names: &mut BTreeSet<String>) {
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
            names.insert(name.clone());
        }
        Predicate::NativeFlag(_) | Predicate::DamageStatusIs(_) | Predicate::EventFlag { .. } => {}
        Predicate::Compare { left, right, .. } => {
            collect_expr(left, names);
            collect_expr(right, names);
        }
        Predicate::Not(inner) => collect_predicate(inner, names),
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            collect_predicate(left, names);
            collect_predicate(right, names);
        }
    }
}
