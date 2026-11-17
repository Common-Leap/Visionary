//! Lower move-graph custom triggers into the constrained fighter behavior graph.
//!
//! Move names are authoring organization only. Every trigger compiled here is
//! character-wide: this lowering does not imply move, status, or animation
//! scoping. Trigger transitions are appended after base transitions and in
//! deterministic move/name order. The existing emitter walks every bound
//! transition as an ordered `if`; this lowering does not promise that a state
//! change stops later transitions in the same hook pass.

use std::collections::{BTreeMap, BTreeSet};

use crate::gimmicks::{
    behavior::{
        self, Action, BehaviorGraph, CmpOp, ConstValue, Expr, Predicate, StateDef, Transition,
        VarDef, VarType,
    },
    codegen::{self, EventBinding, HookBinding, ResourceTrigger},
    hooks::{self, HookName},
};

use super::{
    model::{MoveGraph, NodeError, TriggerSource, VariableScope},
    references,
};

/// Compile named move triggers into the existing character-level behavior IR.
///
/// Variables with identical definitions are shared across move graphs. A
/// timer-expiry trigger uses an `onChargeTick` latch: it arms only while the
/// timer is positive and clears after its first expiry, so a timer held at zero
/// cannot fire continuously. The latch starts armed only when the timer's
/// initial value is positive.
pub fn compile(
    fighter: &str,
    base: &BehaviorGraph,
    graphs: &BTreeMap<String, MoveGraph>,
) -> Result<BehaviorGraph, Vec<NodeError>> {
    // Bound and validate all authoring data before cloning any recursive
    // expression/predicate tree into the lowered behavior graph.
    let mut preflight_errors = Vec::new();
    for (move_name, graph) in graphs {
        if let Err(error) = references::preflight_scopes(graph) {
            preflight_errors.push(node_error(fighter, move_name, "graph", error));
        }
    }
    let authored_trigger_count = graphs.values().fold(0usize, |count, graph| {
        count.saturating_add(graph.triggers.len())
    });
    if authored_trigger_count > crate::gimmicks::live_model::MAX_TRANSITIONS {
        preflight_errors.push(node_error(
            fighter,
            "moves",
            "triggers",
            format!(
                "move recipes contain {authored_trigger_count} triggers, exceeding the aggregate limit {}",
                crate::gimmicks::live_model::MAX_TRANSITIONS
            ),
        ));
    }
    if !preflight_errors.is_empty() {
        return Err(preflight_errors);
    }

    // A routine name denotes one reusable identity across the character's
    // recipe graphs. Repeated definitions must agree on body, signature, and
    // nested calls before any definition is expanded.
    let mut routine_definitions: BTreeMap<String, super::expand::RoutineDefinition<'_>> =
        BTreeMap::new();
    for (move_name, graph) in graphs {
        for (routine_name, body) in &graph.routines {
            let inputs = graph
                .routine_inputs
                .get(routine_name)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let outputs = graph
                .routine_outputs
                .get(routine_name)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let nested_calls = graph
                .routine_calls_in_routines
                .get(routine_name)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if let Some(first) = routine_definitions.get(routine_name) {
                if first.body != body
                    || first.inputs != inputs
                    || first.outputs != outputs
                    || first.calls != nested_calls
                {
                    preflight_errors.push(node_error(
                        fighter,
                        move_name,
                        routine_name,
                        format!(
                            "routine name conflicts with a different definition in {}",
                            first.owner_move
                        ),
                    ));
                }
            } else {
                routine_definitions.insert(
                    routine_name.clone(),
                    super::expand::RoutineDefinition {
                        owner_move: move_name,
                        body,
                        inputs,
                        outputs,
                        calls: nested_calls,
                    },
                );
            }
        }
    }
    let aggregate_routine_actions = routine_definitions
        .values()
        .fold(0usize, |count, definition| {
            count.saturating_add(definition.body.len())
        });
    let routine_limit = crate::gimmicks::live_model::MAX_ACTIONS_PER_TRANSITION
        .saturating_mul(crate::gimmicks::live_model::MAX_TRANSITIONS);
    if routine_definitions.len() > crate::gimmicks::live_model::MAX_TRANSITIONS
        || aggregate_routine_actions > routine_limit
    {
        preflight_errors.push(node_error(
            fighter,
            "moves",
            "routines",
            format!(
                "character routines exceed the aggregate definition/action limit {routine_limit}"
            ),
        ));
    }
    for (move_name, graph) in graphs {
        for (trigger_name, calls) in &graph.routine_calls {
            for call in calls {
                if !routine_definitions.contains_key(&call.routine) {
                    preflight_errors.push(node_error(
                        fighter,
                        move_name,
                        trigger_name,
                        format!("calls missing routine {:?}", call.routine),
                    ));
                }
            }
        }
    }
    if !preflight_errors.is_empty() {
        return Err(preflight_errors);
    }
    // Validate the base graph's recursive IR and emission-plan vectors before
    // any validator below clones its declarations or caller event schemas.
    preflight_base_event_headers(fighter, base)?;
    if let Err(error) = crate::gimmicks::field_projection::project(base) {
        return Err(vec![node_error(fighter, "base", "graph", error)]);
    }

    // Check routine identities first so their conflicts are not obscured by
    // downstream event references. Schemas still pass their complete bounded
    // preflight before any recursive data or field table is cloned.
    let event_schemas = preflight_event_schemas(fighter, base, graphs)?;
    // Keep routine bodies borrowed until bounded plans and whole-character
    // variable accounting succeed. No recursive action clone happens here.
    let definitions = routine_definitions;
    if let Err(error) = super::expand::validate_definition_graph(&definitions) {
        preflight_errors.push(node_error(fighter, "moves", "routines", error));
    }
    if !preflight_errors.is_empty() {
        return Err(preflight_errors);
    }
    let mut expansion_plans = BTreeMap::new();
    for (move_name, graph) in graphs {
        match super::expand::preflight_expansion(fighter, move_name, graph, &definitions) {
            Ok(plan) => {
                expansion_plans.insert(move_name.clone(), plan);
            }
            Err(error) => preflight_errors.push(node_error(fighter, move_name, "routines", error)),
        }
    }
    // Per-call input/output scratch becomes live character state, so account
    // for shared and move-private identities across all recipes before cloning.
    let mut whole_character_vars: BTreeSet<String> =
        base.vars.iter().map(|var| var.name.clone()).collect();
    let mut preflight_local_owners = BTreeMap::new();
    for (move_name, graph) in graphs {
        for var in &graph.variables {
            let name = if graph.scope(&var.name) == VariableScope::MoveLocal {
                match references::runtime_variable_name(move_name, &var.name) {
                    Ok(name) => name,
                    Err(error) => {
                        preflight_errors.push(node_error(fighter, move_name, &var.name, error));
                        continue;
                    }
                }
            } else {
                var.name.clone()
            };
            if graph.scope(&var.name) == VariableScope::MoveLocal {
                if let Some(owner) =
                    preflight_local_owners.insert(name.clone(), (move_name, &var.name))
                {
                    if owner != (move_name, &var.name) {
                        preflight_errors.push(node_error(fighter, move_name, &var.name, "generated move-local variable name collides with another local identity"));
                    }
                }
            }
            whole_character_vars.insert(name);
        }
    }
    let authored_character_vars = whole_character_vars.clone();
    let mut scratch_owners = BTreeSet::new();
    for (move_name, plan) in &expansion_plans {
        for var in &plan.scratch_vars {
            if authored_character_vars.contains(&var.name) {
                preflight_errors.push(node_error(
                    fighter,
                    move_name,
                    &var.name,
                    "routine call scratch collides with an existing variable",
                ));
            }
            if !scratch_owners.insert(var.name.clone()) {
                preflight_errors.push(node_error(
                    fighter,
                    move_name,
                    &var.name,
                    "generated routine call scratch collides with another callsite",
                ));
            }
            whole_character_vars.insert(var.name.clone());
        }
    }
    if whole_character_vars.len() > crate::gimmicks::live_model::MAX_VARIABLES {
        preflight_errors.push(node_error(
            fighter,
            "moves",
            "variables",
            "combined move variables and routine captures/output scratch exceed the character variable budget",
        ));
    }
    if !preflight_errors.is_empty() {
        return Err(preflight_errors);
    }

    // Routine validation builds synthetic BehaviorGraphs by cloning each
    // routine tree and its caller declarations. Defer it until expansion,
    // action-count, scratch-name, and whole-character variable budgets have
    // all passed.
    for (move_name, graph) in graphs {
        if let Err(error) =
            super::expand::validate_owned_routines(fighter, move_name, base, graph, graphs)
        {
            preflight_errors.push(node_error(fighter, move_name, "routines", error));
        }
    }
    if !preflight_errors.is_empty() {
        return Err(preflight_errors);
    }

    let mut out = base.clone();
    let mut errors = Vec::new();
    let mut triggers = Vec::new();

    // Reject duplicate declarations inside one graph, but let identical
    // definitions in different moves refer to one shared character variable.
    let mut vars: BTreeMap<String, VarDef> = out
        .vars
        .iter()
        .cloned()
        .map(|var| (var.name.clone(), var))
        .collect();
    if vars.keys().any(|name| {
        name.starts_with(references::GENERATED_LOCAL_PREFIX)
            || name.starts_with(references::ROUTINE_INPUT_PREFIX)
            || name.starts_with(references::ROUTINE_OUTPUT_PREFIX)
    }) {
        return Err(vec![node_error(
            fighter,
            "base",
            "variables",
            "base behavior uses a reserved move-local namespace or routine-port namespace",
        )]);
    }
    let mut duplicate_base_vars = BTreeSet::new();
    for var in &out.vars {
        if !duplicate_base_vars.insert(var.name.clone()) {
            errors.push(node_error(
                fighter,
                "base",
                &var.name,
                "the base behavior declares this variable more than once",
            ));
        }
    }

    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut local_runtime_owners: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (move_name, graph) in graphs {
        if graph.version != super::model::SCHEMA_VERSION {
            errors.push(node_error(
                fighter,
                move_name,
                "graph",
                format!(
                    "unsupported move graph version {}; expected {}",
                    graph.version,
                    super::model::SCHEMA_VERSION
                ),
            ));
        }
        let mut local_vars = BTreeSet::new();
        let mut local_mapping = BTreeMap::new();
        for var in &graph.variables {
            if !local_vars.insert(var.name.clone()) {
                errors.push(node_error(
                    fighter,
                    move_name,
                    &var.name,
                    "the move graph declares this variable more than once",
                ));
                continue;
            }
            if graph.scope(&var.name) == VariableScope::MoveLocal {
                let runtime_name = match references::runtime_variable_name(move_name, &var.name) {
                    Ok(name) => name,
                    Err(error) => {
                        errors.push(node_error(fighter, move_name, &var.name, error));
                        continue;
                    }
                };
                if let Some(owner) = local_runtime_owners.get(&runtime_name) {
                    if owner != &(move_name.clone(), var.name.clone()) {
                        errors.push(node_error(
                            fighter,
                            move_name,
                            &var.name,
                            "generated move-local variable name collides with another local identity",
                        ));
                        continue;
                    }
                } else {
                    local_runtime_owners
                        .insert(runtime_name.clone(), (move_name.clone(), var.name.clone()));
                }
                if let Some(existing) = vars.get(&runtime_name) {
                    if existing != var {
                        errors.push(node_error(
                            fighter,
                            move_name,
                            &var.name,
                            "generated move-local variable name collides with another definition",
                        ));
                        continue;
                    }
                } else {
                    let mut lowered = var.clone();
                    lowered.name.clone_from(&runtime_name);
                    vars.insert(runtime_name.clone(), lowered.clone());
                    out.vars.push(lowered);
                }
                local_mapping.insert(var.name.clone(), runtime_name);
                continue;
            }

            match vars.get(&var.name) {
                Some(existing) if existing != var => errors.push(node_error(
                    fighter,
                    move_name,
                    &var.name,
                    "this variable conflicts with a different character-wide definition",
                )),
                Some(_) => {}
                None => {
                    vars.insert(var.name.clone(), var.clone());
                    out.vars.push(var.clone());
                }
            }
        }

        let expansion_plan = expansion_plans
            .get(move_name)
            .expect("every bounded move recipe has an expansion plan");
        for scratch in &expansion_plan.scratch_vars {
            if vars.contains_key(&scratch.name) {
                errors.push(node_error(
                    fighter,
                    move_name,
                    &scratch.name,
                    "routine call capture variable collides with an existing character variable",
                ));
                continue;
            }
            vars.insert(scratch.name.clone(), scratch.clone());
            out.vars.push(scratch.clone());
        }

        let expanded_actions =
            match super::expand::expand_trigger_actions(graph, &definitions, expansion_plan) {
                Ok(actions) => actions,
                Err(error) => {
                    errors.push(node_error(fighter, move_name, "routines", error));
                    continue;
                }
            };

        let mut local_trigger_names = BTreeSet::new();
        for (trigger_index, trigger) in graph.triggers.iter().enumerate() {
            let label = format!("{move_name}/{}", trigger.name);
            if trigger.name.trim().is_empty() {
                errors.push(node_error(
                    fighter,
                    move_name,
                    "trigger",
                    "trigger name must not be empty",
                ));
                continue;
            }
            if !local_trigger_names.insert(trigger.name.clone()) {
                errors.push(node_error(
                    fighter,
                    move_name,
                    &trigger.name,
                    "the move graph declares this trigger name more than once",
                ));
                continue;
            }
            if let Some(first) = names.insert(trigger.name.clone(), label.clone()) {
                errors.push(node_error(
                    fighter,
                    move_name,
                    &trigger.name,
                    format!("trigger name is already used by {first}; names are character-wide"),
                ));
                continue;
            }
            if let TriggerSource::Hook(hook) = &trigger.source {
                let has_reviewed_source = base
                    .emission_plan
                    .status_triggers
                    .iter()
                    .any(|source| source.hook == *hook)
                    || base
                        .emission_plan
                        .resource_triggers
                        .iter()
                        .any(|source| source.hook == *hook);
                if let Err(error) = hooks::resolve_binding(hook.name(), has_reviewed_source) {
                    errors.push(node_error(
                        fighter,
                        move_name,
                        &trigger.name,
                        error.to_string(),
                    ));
                    continue;
                }
            }
            let mut lowered_trigger = trigger.clone();
            lowered_trigger.actions = expanded_actions[trigger_index].clone();
            references::remap_trigger_variables(&mut lowered_trigger, &local_mapping);
            triggers.push((move_name.clone(), lowered_trigger));
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    if triggers.is_empty() {
        // A variables-only MoveGraph cannot be attached to an empty behavior:
        // report the invalid result through the same IR validation below.
        return validate_result(fighter, out, &BTreeMap::new());
    }

    if out.states.is_empty() {
        let state = "move_trigger_idle".to_string();
        out.initial = state.clone();
        out.states.push(StateDef {
            status: None,
            name: state,
        });
    }

    // Reject an obviously oversized expansion before creating one transition per
    // state/trigger. The live protocol has a small fixed transition budget, so
    // constructing a much larger intermediate graph cannot produce a usable
    // program and can amplify a malformed saved graph needlessly.
    let state_count = out.states.len();
    let added_transition_count = triggers.iter().fold(0usize, |count, (_, trigger)| {
        let per_state = if matches!(&trigger.source, TriggerSource::TimerExpired { .. }) {
            if trigger.guard.is_some() {
                3
            } else {
                2
            }
        } else {
            1
        };
        count.saturating_add(state_count.saturating_mul(per_state))
    });
    if out.transitions.len().saturating_add(added_transition_count)
        > crate::gimmicks::live_model::MAX_TRANSITIONS
    {
        return Err(vec![NodeError {
            subject: format!("{fighter}/move triggers"),
            reason: format!(
                "expanded trigger graph exceeds the live transition budget ({})",
                crate::gimmicks::live_model::MAX_TRANSITIONS
            ),
        }]);
    }

    // Keep the IR state lookup total even when the caller passes a malformed
    // base; final validation turns those shape problems into NodeErrors.
    let states = out.states.clone();
    let mut hook_indexes: BTreeMap<HookName, Vec<usize>> = BTreeMap::new();
    let mut event_indexes: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut status_indexes: BTreeMap<(HookName, String), Vec<usize>> = BTreeMap::new();
    let mut status_transition_owner: BTreeMap<usize, (HookName, String)> = BTreeMap::new();
    let mut resource_sources: BTreeMap<HookName, ResourceTrigger> = BTreeMap::new();
    let mut occupied_var_names: BTreeSet<String> =
        out.vars.iter().map(|var| var.name.clone()).collect();
    let mut transition_owners: BTreeMap<usize, String> = BTreeMap::new();
    let mut timer_latches: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();

    for (move_name, trigger) in triggers {
        let subject = format!("{move_name}/{}", trigger.name);
        let (hook, resource) = match &trigger.source {
            TriggerSource::Event { name, .. } => {
                if name.trim().is_empty() || name.len() > 48 || name.contains('\0') {
                    errors.push(node_error(
                        fighter,
                        &move_name,
                        &trigger.name,
                        "event name must be non-empty, NUL-free, and at most 48 bytes",
                    ));
                    continue;
                }
                for state in &states {
                    let index = out.transitions.len();
                    transition_owners.insert(index, format!("{fighter}/{subject}"));
                    out.transitions.push(Transition {
                        from: state.name.clone(),
                        to: state.name.clone(),
                        guard: trigger.guard.clone().unwrap_or(Predicate::Compare {
                            op: CmpOp::Eq,
                            left: Expr::Const(ConstValue::Int(0)),
                            right: Expr::Const(ConstValue::Int(0)),
                        }),
                        on_frame: true,
                        actions: trigger.actions.clone(),
                    });
                    event_indexes.entry(name.clone()).or_default().push(index);
                }
                continue;
            }
            TriggerSource::StatusEntered { symbol } => {
                if let Err(error) = hooks::resolve_binding(HookName::OnStatusEnter.name(), true) {
                    errors.push(node_error(
                        fighter,
                        &move_name,
                        &trigger.name,
                        error.to_string(),
                    ));
                    continue;
                }
                for state in &states {
                    let index = out.transitions.len();
                    transition_owners.insert(index, format!("{fighter}/{subject}"));
                    out.transitions.push(Transition {
                        from: state.name.clone(),
                        to: state.name.clone(),
                        guard: trigger.guard.clone().unwrap_or(Predicate::Compare {
                            op: CmpOp::Eq,
                            left: Expr::Const(ConstValue::Int(0)),
                            right: Expr::Const(ConstValue::Int(0)),
                        }),
                        on_frame: true,
                        actions: trigger.actions.clone(),
                    });
                    hook_indexes
                        .entry(HookName::OnStatusEnter)
                        .or_default()
                        .push(index);
                    status_indexes
                        .entry((HookName::OnStatusEnter, symbol.clone()))
                        .or_default()
                        .push(index);
                    status_transition_owner
                        .insert(index, (HookName::OnStatusEnter, symbol.clone()));
                }
                continue;
            }
            TriggerSource::StatusExited { symbol } => {
                if let Err(error) = hooks::resolve_binding(HookName::OnStatusExit.name(), true) {
                    errors.push(node_error(
                        fighter,
                        &move_name,
                        &trigger.name,
                        error.to_string(),
                    ));
                    continue;
                }
                for state in &states {
                    let index = out.transitions.len();
                    transition_owners.insert(index, format!("{fighter}/{subject}"));
                    out.transitions.push(Transition {
                        from: state.name.clone(),
                        to: state.name.clone(),
                        guard: trigger.guard.clone().unwrap_or(Predicate::Compare {
                            op: CmpOp::Eq,
                            left: Expr::Const(ConstValue::Int(0)),
                            right: Expr::Const(ConstValue::Int(0)),
                        }),
                        on_frame: true,
                        actions: trigger.actions.clone(),
                    });
                    hook_indexes
                        .entry(HookName::OnStatusExit)
                        .or_default()
                        .push(index);
                    status_indexes
                        .entry((HookName::OnStatusExit, symbol.clone()))
                        .or_default()
                        .push(index);
                    status_transition_owner.insert(index, (HookName::OnStatusExit, symbol.clone()));
                }
                continue;
            }
            TriggerSource::Hook(hook) => (*hook, None),
            TriggerSource::TimerExpired { timer } => {
                match vars.get(timer) {
                    Some(var) if var.kind == VarType::Timer => {}
                    Some(var) => {
                        errors.push(node_error(
                            fighter,
                            &move_name,
                            &trigger.name,
                            format!(
                                "timer source {timer:?} refers to a {:?}, not a timer",
                                var.kind
                            ),
                        ));
                        continue;
                    }
                    None => {
                        errors.push(node_error(
                            fighter,
                            &move_name,
                            &trigger.name,
                            format!("timer source {timer:?} is not declared in the base or move variables"),
                        ));
                        continue;
                    }
                }
                let latch = unique_latch_name(&trigger.name, &mut occupied_var_names);
                let initially_armed = matches!(vars.get(timer).map(|var| var.initial), Some(ConstValue::Int(frames)) if frames > 0);
                let latch_var = VarDef {
                    keep_across_form_switch: false,
                    name: latch.clone(),
                    kind: VarType::Flag,
                    initial: ConstValue::Flag(initially_armed),
                };
                vars.insert(latch.clone(), latch_var.clone());
                out.vars.push(latch_var);
                timer_latches
                    .entry(timer.clone())
                    .or_default()
                    .push((latch.clone(), subject.clone()));

                for state in &states {
                    // Re-arm after any positive timer restart. This transition
                    // precedes the expiry edge for this trigger on frame ticks.
                    let index = out.transitions.len();
                    transition_owners.insert(index, format!("{fighter}/{subject}"));
                    out.transitions.push(Transition {
                        from: state.name.clone(),
                        to: state.name.clone(),
                        guard: Predicate::Not(Box::new(Predicate::Expired(timer.clone()))),
                        on_frame: true,
                        actions: vec![Action::Raise { var: latch.clone() }],
                    });
                    hook_indexes
                        .entry(HookName::OnChargeTick)
                        .or_default()
                        .push(index);
                }

                let mut expiry_guard = Predicate::And(
                    Box::new(Predicate::Expired(timer.clone())),
                    Box::new(Predicate::Flag(latch.clone())),
                );
                if let Some(guard) = trigger.guard.clone() {
                    expiry_guard = Predicate::And(Box::new(expiry_guard), Box::new(guard));
                }
                let mut actions = trigger.actions.clone();
                actions.push(Action::Clear { var: latch.clone() });
                for state in &states {
                    let index = out.transitions.len();
                    transition_owners.insert(index, format!("{fighter}/{subject}"));
                    out.transitions.push(Transition {
                        from: state.name.clone(),
                        to: state.name.clone(),
                        guard: expiry_guard.clone(),
                        on_frame: true,
                        actions: actions.clone(),
                    });
                    hook_indexes
                        .entry(HookName::OnChargeTick)
                        .or_default()
                        .push(index);
                }
                if let Some(guard) = trigger.guard.clone() {
                    // The optional predicate is sampled on the expiry frame.
                    // If it is false, consume the latch so a later change to
                    // that predicate cannot turn a past expiry into a new
                    // firing while the timer remains at zero.
                    let missed_guard = Predicate::And(
                        Box::new(Predicate::Expired(timer.clone())),
                        Box::new(Predicate::And(
                            Box::new(Predicate::Flag(latch.clone())),
                            Box::new(Predicate::Not(Box::new(guard))),
                        )),
                    );
                    for state in &states {
                        let index = out.transitions.len();
                        transition_owners.insert(index, format!("{fighter}/{subject}"));
                        out.transitions.push(Transition {
                            from: state.name.clone(),
                            to: state.name.clone(),
                            guard: missed_guard.clone(),
                            on_frame: true,
                            actions: vec![Action::Clear { var: latch.clone() }],
                        });
                        hook_indexes
                            .entry(HookName::OnChargeTick)
                            .or_default()
                            .push(index);
                    }
                }
                continue;
            }
            TriggerSource::Resource(source) => {
                if source.fighter() != fighter {
                    errors.push(node_error(
                        fighter,
                        &move_name,
                        &trigger.name,
                        format!(
                            "{} belongs to {}, not {fighter}",
                            source.label(),
                            source.fighter()
                        ),
                    ));
                    continue;
                }
                let hook = source.hook();
                (hook, Some(*source))
            }
        };

        if let Some(source) = resource {
            if let Some(existing) = resource_sources.get(&hook) {
                if existing.source != source {
                    errors.push(node_error(
                        fighter,
                        &move_name,
                        &trigger.name,
                        format!(
                            "{} already uses {}; this trigger requests {}",
                            hook.name(),
                            existing.source.label(),
                            source.label()
                        ),
                    ));
                    continue;
                }
            }
            if let Some(existing) = out
                .emission_plan
                .resource_triggers
                .iter()
                .find(|existing| existing.hook == hook && existing.source != source)
            {
                errors.push(node_error(
                    fighter,
                    &move_name,
                    &trigger.name,
                    format!(
                        "{} already binds {}; the move trigger requests {}",
                        hook.name(),
                        existing.source.label(),
                        source.label()
                    ),
                ));
                continue;
            }
            resource_sources
                .entry(hook)
                .or_insert(ResourceTrigger { hook, source });
        }

        for state in &states {
            let guard = trigger.guard.clone().unwrap_or(Predicate::Compare {
                op: CmpOp::Eq,
                left: Expr::Const(ConstValue::Int(0)),
                right: Expr::Const(ConstValue::Int(0)),
            });
            let index = out.transitions.len();
            transition_owners.insert(index, format!("{fighter}/{subject}"));
            out.transitions.push(Transition {
                from: state.name.clone(),
                to: state.name.clone(),
                guard,
                on_frame: true,
                actions: trigger.actions.clone(),
            });
            hook_indexes.entry(hook).or_default().push(index);
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    // An authored Set/Reset may start or cancel a one-frame timer on a
    // non-frame hook. Mark the write from inside the original transition, so
    // its helpers do nothing when that transition's guard was false. The
    // immediately following Expired/Not(Expired) edges clear or arm latches.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    enum SourceKey {
        Hook(HookName),
        Event(String),
    }
    let mut ordered_indexes: BTreeMap<SourceKey, Vec<usize>> = BTreeMap::new();
    for binding in &out.emission_plan.bindings {
        ordered_indexes.insert(SourceKey::Hook(binding.hook), binding.transitions.clone());
    }
    for binding in &out.emission_plan.event_bindings {
        ordered_indexes.insert(
            SourceKey::Event(binding.name.clone()),
            binding.transitions.clone(),
        );
    }
    for (hook, indexes) in &hook_indexes {
        ordered_indexes
            .entry(SourceKey::Hook(*hook))
            .or_default()
            .extend(indexes.iter().copied());
    }
    for (name, indexes) in &event_indexes {
        ordered_indexes
            .entry(SourceKey::Event(name.clone()))
            .or_default()
            .extend(indexes.iter().copied());
    }
    let mut helper_transition_count = 0usize;
    for indexes in ordered_indexes.values() {
        for index in indexes {
            for (timer, latches) in &timer_latches {
                let starts_timer = out.transitions.get(*index).is_some_and(|transition| {
                    transition.actions.iter().any(|action| match action {
                        Action::Set { var, .. }
                        | Action::SetFlag { var, .. }
                        | Action::Reset { var } => var == timer,
                        _ => false,
                    })
                });
                if starts_timer {
                    helper_transition_count = helper_transition_count.saturating_add(
                        states.len().saturating_mul(latches.len()).saturating_mul(2),
                    );
                }
            }
        }
    }
    if out
        .transitions
        .len()
        .saturating_add(helper_transition_count)
        > crate::gimmicks::live_model::MAX_TRANSITIONS
    {
        return Err(vec![NodeError {
            subject: format!("{fighter}/move triggers"),
            reason: format!(
                "timer latch helpers exceed the live transition budget ({})",
                crate::gimmicks::live_model::MAX_TRANSITIONS
            ),
        }]);
    }
    let mut latch_updates_after_transition: BTreeMap<(SourceKey, usize), Vec<usize>> =
        BTreeMap::new();
    for (hook, indexes) in &ordered_indexes {
        for index in indexes {
            for (timer, latches) in &timer_latches {
                let starts_timer = out.transitions.get(*index).is_some_and(|transition| {
                    transition.actions.iter().any(|action| match action {
                        Action::Set { var, .. }
                        | Action::SetFlag { var, .. }
                        | Action::Reset { var } => var == timer,
                        _ => false,
                    })
                });
                if !starts_timer {
                    continue;
                }
                for (latch, owner) in latches {
                    let marker = unique_latch_name(
                        &format!(
                            "writer_{}_{}_{}",
                            match hook {
                                SourceKey::Hook(hook) => hook.name().to_string(),
                                SourceKey::Event(name) => format!("event_{name}"),
                            },
                            index,
                            latch
                        ),
                        &mut occupied_var_names,
                    );
                    out.vars.push(VarDef {
                        keep_across_form_switch: false,
                        name: marker.clone(),
                        kind: VarType::Flag,
                        initial: ConstValue::Flag(false),
                    });
                    if let Some(transition) = out.transitions.get_mut(*index) {
                        transition.actions.push(Action::Raise {
                            var: marker.clone(),
                        });
                    }
                    for state in &states {
                        // A zero write cancels any pending expiry. For a
                        // positive write this guard is false, so an existing
                        // countdown latch is left armed until its own edge.
                        // The marker also proves the writer transition fired.
                        let clear_index = out.transitions.len();
                        transition_owners.insert(clear_index, format!("{fighter}/{owner}"));
                        out.transitions.push(Transition {
                            from: state.name.clone(),
                            to: state.name.clone(),
                            guard: Predicate::And(
                                Box::new(Predicate::Flag(marker.clone())),
                                Box::new(Predicate::Expired(timer.clone())),
                            ),
                            on_frame: true,
                            actions: vec![
                                Action::Clear { var: latch.clone() },
                                Action::Clear {
                                    var: marker.clone(),
                                },
                            ],
                        });
                        latch_updates_after_transition
                            .entry((hook.clone(), *index))
                            .or_default()
                            .push(clear_index);

                        let arm_index = out.transitions.len();
                        transition_owners.insert(arm_index, format!("{fighter}/{owner}"));
                        out.transitions.push(Transition {
                            from: state.name.clone(),
                            to: state.name.clone(),
                            guard: Predicate::And(
                                Box::new(Predicate::Flag(marker.clone())),
                                Box::new(Predicate::Not(Box::new(Predicate::Expired(
                                    timer.clone(),
                                )))),
                            ),
                            on_frame: true,
                            actions: vec![
                                Action::Raise { var: latch.clone() },
                                Action::Clear {
                                    var: marker.clone(),
                                },
                            ],
                        });
                        latch_updates_after_transition
                            .entry((hook.clone(), *index))
                            .or_default()
                            .push(arm_index);
                    }
                }
            }
        }
    }
    for (source, indexes) in ordered_indexes {
        let mut ordered = Vec::new();
        for index in indexes {
            ordered.push(index);
            if let Some(updates) = latch_updates_after_transition.remove(&(source.clone(), index)) {
                if let Some(status_key) = status_transition_owner.get(&index) {
                    status_indexes
                        .entry(status_key.clone())
                        .or_default()
                        .extend(updates.iter().copied());
                }
                ordered.extend(updates);
            }
        }
        match source {
            SourceKey::Hook(hook) => {
                if let Some(binding) = out
                    .emission_plan
                    .bindings
                    .iter_mut()
                    .find(|item| item.hook == hook)
                {
                    binding.transitions = ordered;
                } else {
                    out.emission_plan.bindings.push(HookBinding {
                        hook,
                        article: None,
                        transitions: ordered,
                    });
                }
            }
            SourceKey::Event(name) => {
                if let Some(binding) = out
                    .emission_plan
                    .event_bindings
                    .iter_mut()
                    .find(|item| item.name == name)
                {
                    binding.transitions = ordered;
                } else {
                    let fields = event_schemas.get(&name).cloned().unwrap_or_default();
                    out.emission_plan.event_bindings.push(EventBinding {
                        name,
                        transitions: ordered,
                        fields,
                    });
                }
            }
        }
    }
    for (_, trigger) in resource_sources {
        if !out
            .emission_plan
            .resource_triggers
            .iter()
            .any(|item| item.hook == trigger.hook)
        {
            out.emission_plan.resource_triggers.push(trigger);
        }
    }
    for ((hook, symbol), indexes) in status_indexes {
        let binding_order = out
            .emission_plan
            .bindings
            .iter()
            .find(|binding| binding.hook == hook)
            .map(|binding| binding.transitions.clone())
            .unwrap_or_default();
        let selected: BTreeSet<_> = indexes.into_iter().collect();
        let transitions: Vec<_> = binding_order
            .iter()
            .filter(|index| selected.contains(index))
            .copied()
            .collect();
        if let Some(existing) = out
            .emission_plan
            .status_triggers
            .iter_mut()
            .find(|item| item.hook == hook && item.symbol == symbol)
        {
            existing.transitions.extend(transitions);
            existing.transitions.dedup();
        } else {
            out.emission_plan
                .status_triggers
                .push(codegen::StatusTrigger {
                    hook,
                    symbol,
                    evidence: String::new(),
                    runtime_validated: false,
                    transitions,
                });
        }
    }

    validate_result(fighter, out, &transition_owners)
}

fn validate_result(
    fighter: &str,
    graph: BehaviorGraph,
    owners: &BTreeMap<usize, String>,
) -> Result<BehaviorGraph, Vec<NodeError>> {
    let mut errors = Vec::new();
    errors.extend(
        behavior::validate_for_fighter(fighter, &graph)
            .into_iter()
            .map(|error| {
                let owner = error
                    .item
                    .strip_prefix("transition ")
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|index| index.parse::<usize>().ok())
                    .and_then(|index| owners.get(&index));
                NodeError {
                    subject: owner
                        .cloned()
                        .unwrap_or_else(|| format!("{fighter}/base behavior")),
                    reason: error.to_string(),
                }
            }),
    );
    if !graph.transitions.is_empty() || !graph.emission_plan.is_empty() {
        errors.extend(
            codegen::check_plan(&graph, &graph.emission_plan)
                .into_iter()
                .map(|error| NodeError {
                    subject: format!("{fighter}/emission plan"),
                    reason: error.to_string(),
                }),
        );
    }
    errors.extend(
        codegen::check_status_triggers(fighter, &graph.emission_plan)
            .into_iter()
            .map(|error| NodeError {
                subject: format!("{fighter}/emission plan"),
                reason: error.to_string(),
            }),
    );
    if !graph.is_empty() {
        if let Err(reason) = crate::gimmicks::live_protocol::program_from_graph(fighter, &graph) {
            errors.push(NodeError {
                subject: format!("{fighter}/live behavior"),
                reason,
            });
        }
    }
    if errors.is_empty() {
        Ok(graph)
    } else {
        Err(errors)
    }
}

fn unique_latch_name(trigger: &str, occupied: &mut BTreeSet<String>) -> String {
    let normalized: String = trigger
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    // Keep generated names within the live protocol's 48-byte variable-name
    // bound even when an author supplies a very long or non-ASCII name.
    let stem: String = normalized.chars().take(20).collect();
    let hash = trigger.bytes().fold(0x811c_9dc5u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    let base = format!("__mta_{stem}_{hash:08x}");
    if occupied.insert(base.clone()) {
        return base;
    }
    let mut suffix = 2usize;
    loop {
        let candidate = format!("{base}_{suffix}");
        if occupied.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

/// Validate receiver declarations without mutating move graphs or the base.
/// The trigger-count preflight runs before this helper. Base binding count is
/// checked before traversal; each schema is length-checked before any field
/// iteration or clone. The pass is therefore bounded by existing graph
/// limits plus at most eight fields per declared event.
fn preflight_base_event_headers(fighter: &str, base: &BehaviorGraph) -> Result<(), Vec<NodeError>> {
    if base.emission_plan.event_bindings.len() > codegen::MAX_EVENT_BINDINGS {
        return Err(vec![node_error(
            fighter,
            "base",
            "event bindings",
            format!(
                "base has {} event bindings; maximum is {}",
                base.emission_plan.event_bindings.len(),
                codegen::MAX_EVENT_BINDINGS
            ),
        )]);
    }
    for binding in &base.emission_plan.event_bindings {
        if binding.name.len() > 48 || binding.name.trim().is_empty() || binding.name.contains('\0')
        {
            return Err(vec![node_error(
                fighter,
                "base",
                "event binding",
                "base event name must be non-empty, NUL-free, and at most 48 bytes",
            )]);
        }
    }
    Ok(())
}

fn preflight_event_schemas(
    fighter: &str,
    base: &BehaviorGraph,
    graphs: &BTreeMap<String, MoveGraph>,
) -> Result<BTreeMap<String, Vec<crate::gimmicks::event_payload::FieldDef>>, Vec<NodeError>> {
    use crate::gimmicks::event_payload::{validate_schema, FieldDef};
    preflight_base_event_headers(fighter, base)?;

    let mut base_schemas: BTreeMap<String, Vec<FieldDef>> = BTreeMap::new();
    for binding in &base.emission_plan.event_bindings {
        if let Err(schema_errors) = validate_schema("base", &binding.name, &binding.fields) {
            let reason = schema_errors
                .first()
                .map(ToString::to_string)
                .unwrap_or_else(|| "base event schema is invalid".into());
            return Err(vec![node_error(fighter, "base", "event binding", reason)]);
        }
        if base_schemas.contains_key(&binding.name) {
            return Err(vec![node_error(
                fighter,
                "base",
                "event binding",
                "base behavior declares a named-event receiver more than once",
            )]);
        }
        // `validate_schema` established the eight-field cap before this clone.
        base_schemas.insert(binding.name.clone(), binding.fields.clone());
    }

    let mut declared: BTreeMap<String, Vec<FieldDef>> = BTreeMap::new();
    let mut errors = Vec::new();
    for (move_name, graph) in graphs {
        for trigger in &graph.triggers {
            let TriggerSource::Event { name, fields } = &trigger.source else {
                continue;
            };
            if name.len() > 48 || name.trim().is_empty() || name.contains('\0') {
                errors.push(node_error(
                    fighter,
                    "moves",
                    "event receiver",
                    "event name must be non-empty, NUL-free, and at most 48 bytes",
                ));
                return Err(errors);
            }
            if let Err(schema_errors) = validate_schema(move_name, name, fields) {
                if let Some(error) = schema_errors.first() {
                    errors.push(node_error(
                        fighter,
                        "moves",
                        "event receiver",
                        error.to_string(),
                    ));
                }
                return Err(errors);
            }
            if let Some(first_fields) = declared.get(name) {
                if first_fields != fields {
                    errors.push(node_error(
                        fighter,
                        "moves",
                        name,
                        "same-name receiver schemas must match in field name, kind, and order",
                    ));
                    return Err(errors);
                }
            } else {
                declared.insert(name.clone(), fields.clone());
                let merged_count = base_schemas.len().saturating_add(
                    declared
                        .keys()
                        .filter(|event| !base_schemas.contains_key(*event))
                        .count(),
                );
                if merged_count > codegen::MAX_EVENT_BINDINGS {
                    return Err(vec![node_error(
                        fighter,
                        "moves",
                        "event bindings",
                        format!(
                            "base and recipe event bindings exceed the limit {}",
                            codegen::MAX_EVENT_BINDINGS
                        ),
                    )]);
                }
            }
        }
    }

    for (event, fields) in &declared {
        if let Some(base_fields) = base_schemas.get(event) {
            if base_fields != fields {
                return Err(vec![node_error(
                    fighter,
                    "base",
                    event,
                    "receiver schema differs from the base event binding; update the authored receiver and base schema together",
                )]);
            }
        }
    }

    let mut all_schemas = base_schemas;
    for (event, fields) in declared {
        all_schemas.entry(event).or_insert(fields);
    }

    // Prove every event send's receiver and payload shape/family against the
    // exact merged table before any trigger or routine action is cloned. Full
    // Numeric expression and receiver-scope typing runs on the bounded lowered
    // graph in `validate_result` below.
    for (transition_index, transition) in base.transitions.iter().enumerate() {
        validate_event_sends(
            fighter,
            "base behavior",
            &format!("transition {transition_index}"),
            &transition.actions,
            &all_schemas,
        )?;
    }
    for (move_name, graph) in graphs {
        for trigger in &graph.triggers {
            validate_event_sends(
                fighter,
                move_name,
                &trigger.name,
                &trigger.actions,
                &all_schemas,
            )?;
        }
        for (routine_name, actions) in &graph.routines {
            validate_event_sends(fighter, move_name, routine_name, actions, &all_schemas)?;
        }
    }

    Ok(all_schemas)
}

fn validate_event_sends(
    fighter: &str,
    owner: &str,
    source: &str,
    actions: &[Action],
    schemas: &BTreeMap<String, Vec<crate::gimmicks::event_payload::FieldDef>>,
) -> Result<(), Vec<NodeError>> {
    use crate::gimmicks::event_payload::canonicalize_arguments;

    for (index, action) in actions.iter().enumerate() {
        let item = format!("{source} action {index}");
        match action {
            Action::EmitEvent { name } => {
                let Some(fields) = schemas.get(name) else {
                    return Err(vec![node_error(
                        fighter,
                        owner,
                        &item,
                        "emits an unbound named event",
                    )]);
                };
                if !fields.is_empty() {
                    return Err(vec![node_error(
                        fighter,
                        owner,
                        &item,
                        "name-only event send cannot target a receiver with fields",
                    )]);
                }
            }
            Action::EmitEventArguments { name, arguments } => {
                let Some(fields) = schemas.get(name) else {
                    return Err(vec![node_error(
                        fighter,
                        owner,
                        &item,
                        "emits an unbound named event",
                    )]);
                };
                if fields.is_empty() {
                    return Err(vec![node_error(
                        fighter,
                        owner,
                        &item,
                        "typed event arguments require a nonempty receiver schema; use the legacy send for zero-field receivers",
                    )]);
                }
                canonicalize_arguments(owner, name, fields, arguments).map_err(|errors| {
                    errors
                        .into_iter()
                        .map(|error| node_error(fighter, owner, &item, error.to_string()))
                        .collect::<Vec<_>>()
                })?;
            }
            Action::EmitEventValues { name, arguments } => {
                let Some(fields) = schemas.get(name) else {
                    return Err(vec![node_error(
                        fighter,
                        owner,
                        &item,
                        "emits an unbound named event",
                    )]);
                };
                if fields.is_empty() {
                    return Err(vec![node_error(
                        fighter,
                        owner,
                        &item,
                        "dynamic event values require a nonempty receiver schema",
                    )]);
                }
                crate::gimmicks::event_payload::validate_value_argument_shape(
                    owner, name, arguments,
                )
                .map_err(|errors| {
                    errors
                        .into_iter()
                        .map(|error| node_error(fighter, owner, &item, error.to_string()))
                        .collect::<Vec<_>>()
                })?;
                if arguments.len() != fields.len() {
                    return Err(vec![node_error(
                        fighter,
                        owner,
                        &item,
                        format!(
                            "received {} arguments for {} declared fields",
                            arguments.len(),
                            fields.len()
                        ),
                    )]);
                }
                for argument in arguments {
                    let Some(field) = fields.iter().find(|field| field.name == argument.field)
                    else {
                        return Err(vec![node_error(
                            fighter,
                            owner,
                            &item,
                            format!("argument field {:?} is not declared", argument.field),
                        )]);
                    };
                    let family_matches = match &argument.value {
                        crate::gimmicks::event_payload::EventValue::Constant(
                            crate::gimmicks::behavior::ConstValue::Float(_),
                        ) => field.kind == VarType::Float,
                        crate::gimmicks::event_payload::EventValue::Constant(
                            crate::gimmicks::behavior::ConstValue::Int(_),
                        ) => field.kind == VarType::Int,
                        crate::gimmicks::event_payload::EventValue::Constant(
                            crate::gimmicks::behavior::ConstValue::Flag(_),
                        )
                        | crate::gimmicks::event_payload::EventValue::Flag(_) => {
                            field.kind == VarType::Flag
                        }
                        crate::gimmicks::event_payload::EventValue::Numeric(_) => {
                            matches!(field.kind, VarType::Int | VarType::Float)
                        }
                    };
                    if !family_matches {
                        return Err(vec![node_error(
                            fighter,
                            owner,
                            &item,
                            format!(
                                "dynamic argument for field {:?} is incompatible with {:?}",
                                field.name, field.kind
                            ),
                        )]);
                    }
                }
                if fields.iter().any(|field| {
                    !arguments
                        .iter()
                        .any(|argument| argument.field == field.name)
                }) {
                    return Err(vec![node_error(
                        fighter,
                        owner,
                        &item,
                        "a required dynamic event field is missing",
                    )]);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn node_error(
    fighter: &str,
    move_name: &str,
    trigger: &str,
    reason: impl Into<String>,
) -> NodeError {
    NodeError {
        subject: format!("{fighter}/{move_name}/{trigger}"),
        reason: reason.into(),
    }
}
