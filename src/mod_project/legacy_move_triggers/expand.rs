//! Bounded expansion of legacy synchronous routine calls during project loading.

use std::collections::{BTreeMap, BTreeSet};

use crate::gimmicks::{behavior::Action, live_model::MAX_ACTIONS_PER_TRANSITION};

use super::{
    model::{
        MoveGraph, RoutineArgument, RoutineArgumentValue, RoutineCall, RoutineInput, RoutineOutput,
        RoutineOutputBinding, TriggerSource, VariableScope,
    },
    references,
};

pub(super) const MAX_ROUTINE_DEPTH: usize = 8;

pub(super) struct RoutineDefinition<'a> {
    pub owner_move: &'a str,
    pub body: &'a [Action],
    pub inputs: &'a [RoutineInput],
    pub outputs: &'a [RoutineOutput],
    pub calls: &'a [RoutineCall],
}

#[derive(Debug)]
pub(super) struct PlannedInput {
    pub name: String,
    pub scratch: String,
}

#[derive(Debug)]
pub(super) struct PlannedOutput {
    pub name: String,
    pub scratch: String,
    pub destination: String,
    pub kind: crate::gimmicks::behavior::VarType,
}

#[derive(Debug)]
pub(super) struct PlannedCall {
    pub routine: String,
    pub inputs: Vec<PlannedInput>,
    pub outputs: Vec<PlannedOutput>,
    pub children: Vec<PlannedCall>,
}

#[derive(Debug, Default)]
pub(super) struct ExpansionPlan {
    pub calls_by_trigger: BTreeMap<String, Vec<PlannedCall>>,
    pub scratch_vars: Vec<crate::gimmicks::behavior::VarDef>,
}

struct ExpansionPlanner<'state> {
    fighter: &'state str,
    move_name: &'state str,
    trigger: &'state str,
    plan: &'state mut ExpansionPlan,
    scratch_names: &'state mut BTreeSet<String>,
    scratch_count: &'state mut usize,
    planned_action_count: &'state mut usize,
    planned_call_count: &'state mut usize,
}

impl ExpansionPlanner<'_> {
    fn plan_call(
        &mut self,
        definitions: &BTreeMap<String, RoutineDefinition<'_>>,
        call: &RoutineCall,
        path: Vec<usize>,
        depth: usize,
    ) -> Result<PlannedCall, String> {
        use crate::gimmicks::behavior::{ConstValue, VarType};
        if depth > MAX_ROUTINE_DEPTH {
            return Err(format!(
                "routine call nesting exceeds depth limit {MAX_ROUTINE_DEPTH}"
            ));
        }
        *self.planned_call_count = self.planned_call_count.saturating_add(1);
        if *self.planned_call_count > MAX_ACTIONS_PER_TRANSITION {
            return Err(format!("routine call expansion at {:?} exceeds call-site limit {MAX_ACTIONS_PER_TRANSITION}", self.trigger));
        }
        let definition = definitions.get(&call.routine).ok_or_else(|| {
            format!(
                "call path {path:?} calls missing routine {:?}",
                call.routine
            )
        })?;
        let arguments = canonical_arguments(&call.routine, call, definition.inputs)?;
        let output_bindings = canonical_outputs(&call.routine, call, definition.outputs)?;
        let output_action_count = definition
            .outputs
            .len()
            .checked_mul(2)
            .ok_or_else(|| "expanded routine action count overflowed".to_owned())?;
        *self.planned_action_count = self
            .planned_action_count
            .checked_add(definition.body.len())
            .and_then(|count| count.checked_add(definition.inputs.len()))
            .and_then(|count| count.checked_add(output_action_count))
            .ok_or_else(|| "expanded routine action count overflowed".to_owned())?;
        if *self.planned_action_count > MAX_ACTIONS_PER_TRANSITION {
            return Err(format!("routine expansion including routine input captures at {:?}/{path:?} exceeds the live action limit {MAX_ACTIONS_PER_TRANSITION}", self.trigger));
        }
        let mut inputs = Vec::with_capacity(definition.inputs.len());
        for (input, _argument) in definition.inputs.iter().zip(arguments) {
            let scratch = references::routine_argument_variable_name_for_path(
                self.fighter,
                self.move_name,
                self.trigger,
                &path,
                &call.routine,
                &input.name,
            )?;
            if !self.scratch_names.insert(scratch.clone()) {
                return Err(format!(
                    "routine call scratch identifier collision at {:?}/{path:?}",
                    self.trigger
                ));
            }
            let initial = match input.kind {
                VarType::Int => ConstValue::Int(0),
                VarType::Float => ConstValue::Float(0.0),
                VarType::Flag => ConstValue::Flag(false),
                VarType::Timer => {
                    return Err(format!(
                        "routine input {:?} in {:?} cannot be a Timer",
                        input.name, definition.owner_move
                    ))
                }
            };
            self.plan
                .scratch_vars
                .push(crate::gimmicks::behavior::VarDef {
                    keep_across_form_switch: false,
                    name: scratch.clone(),
                    kind: input.kind,
                    initial,
                });
            inputs.push(PlannedInput {
                name: input.name.clone(),
                scratch,
            });
            *self.scratch_count = self.scratch_count.saturating_add(1);
        }
        let mut outputs = Vec::with_capacity(definition.outputs.len());
        for (output, binding) in definition.outputs.iter().zip(output_bindings) {
            let scratch = references::routine_output_variable_name_for_path(
                self.fighter,
                self.move_name,
                self.trigger,
                &path,
                &call.routine,
                &output.name,
            )?;
            if !self.scratch_names.insert(scratch.clone()) {
                return Err(format!(
                    "routine output scratch identifier collision at {:?}/{path:?}",
                    self.trigger
                ));
            }
            let initial = match output.kind {
                VarType::Int => ConstValue::Int(0),
                VarType::Float => ConstValue::Float(0.0),
                VarType::Flag => ConstValue::Flag(false),
                VarType::Timer => {
                    return Err(format!(
                        "routine output {:?} in {:?} cannot be a Timer",
                        output.name, definition.owner_move
                    ))
                }
            };
            self.plan
                .scratch_vars
                .push(crate::gimmicks::behavior::VarDef {
                    keep_across_form_switch: false,
                    name: scratch.clone(),
                    kind: output.kind,
                    initial,
                });
            outputs.push(PlannedOutput {
                name: output.name.clone(),
                scratch,
                destination: binding.destination.clone(),
                kind: output.kind,
            });
            *self.scratch_count = self.scratch_count.saturating_add(1);
        }
        let mut children = Vec::with_capacity(definition.calls.len());
        for (child_index, child) in definition.calls.iter().enumerate() {
            let mut child_path = path.clone();
            child_path.push(child_index);
            children.push(self.plan_call(definitions, child, child_path, depth + 1)?);
        }
        Ok(PlannedCall {
            routine: call.routine.clone(),
            inputs,
            outputs,
            children,
        })
    }
}

/// Validate every routine definition, including unused definitions, before
/// any expansion work can clone bodies or expressions.
pub(super) fn validate_definition_graph(
    definitions: &BTreeMap<String, RoutineDefinition<'_>>,
) -> Result<(), String> {
    fn visit(
        name: &str,
        stack: &mut Vec<String>,
        heights: &mut BTreeMap<String, usize>,
        definitions: &BTreeMap<String, RoutineDefinition<'_>>,
    ) -> Result<usize, String> {
        if stack.len() >= MAX_ROUTINE_DEPTH {
            return Err(format!(
                "routine call graph exceeds depth limit {MAX_ROUTINE_DEPTH} at {name:?}"
            ));
        }
        if let Some(height) = heights.get(name) {
            return Ok(*height);
        }
        if stack.iter().any(|current| current == name) {
            let mut cycle = stack.clone();
            cycle.push(name.to_owned());
            return Err(format!(
                "routine call graph contains a cycle: {}",
                cycle.join(" -> ")
            ));
        }
        let definition = definitions
            .get(name)
            .ok_or_else(|| format!("calls missing routine {name:?}"))?;
        stack.push(name.to_owned());
        let mut height = 1usize;
        for call in definition.calls {
            let callee = definitions.get(&call.routine).ok_or_else(|| {
                format!("routine {name:?} calls missing routine {:?}", call.routine)
            })?;
            if call.before > definition.body.len() {
                return Err(format!(
                    "routine {name:?} has invalid call anchor {}",
                    call.before
                ));
            }
            canonical_arguments(&call.routine, call, callee.inputs)?;
            canonical_outputs(&call.routine, call, callee.outputs)?;
            height =
                height.max(visit(&call.routine, stack, heights, definitions)?.saturating_add(1));
        }
        stack.pop();
        if height > MAX_ROUTINE_DEPTH {
            return Err(format!(
                "routine call graph exceeds depth limit {MAX_ROUTINE_DEPTH} at {name:?}"
            ));
        }
        heights.insert(name.to_owned(), height);
        Ok(height)
    }
    let mut heights = BTreeMap::new();
    for name in definitions.keys() {
        visit(name, &mut Vec::new(), &mut heights, definitions)?;
    }
    Ok(())
}

/// A row in the trigger's displayed ordered sequence.
pub(super) fn preflight_expansion(
    fighter: &str,
    move_name: &str,
    graph: &MoveGraph,
    definitions: &BTreeMap<String, RoutineDefinition<'_>>,
) -> Result<ExpansionPlan, String> {
    let mut plan = ExpansionPlan::default();
    let mut scratch_names = BTreeSet::new();
    let mut scratch_count = 0usize;
    for trigger in &graph.triggers {
        let calls = graph
            .routine_calls
            .get(&trigger.name)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut expanded_len = trigger.actions.len();
        let timer_allowance = usize::from(matches!(
            &trigger.source,
            TriggerSource::TimerExpired { .. }
        ));
        let mut planned_action_count = expanded_len.saturating_add(timer_allowance);
        let mut planned_call_count = 0usize;
        let mut planned_calls = Vec::with_capacity(calls.len());
        for (call_index, call) in calls.iter().enumerate() {
            let planned = ExpansionPlanner {
                fighter,
                move_name,
                trigger: &trigger.name,
                plan: &mut plan,
                scratch_names: &mut scratch_names,
                scratch_count: &mut scratch_count,
                planned_action_count: &mut planned_action_count,
                planned_call_count: &mut planned_call_count,
            }
            .plan_call(definitions, call, vec![call_index], 1)?;
            expanded_len = expanded_len
                .checked_add(call_expanded_len(&planned, definitions)?)
                .and_then(|len| len.checked_add(planned.inputs.len()))
                .ok_or_else(|| "expanded routine action count overflowed".to_owned())?;
            planned_calls.push(planned);
        }
        let allowance = timer_allowance;
        if expanded_len.saturating_add(allowance) > MAX_ACTIONS_PER_TRANSITION {
            return Err(format!(
                "trigger {:?} expands to {expanded_len} actions including routine captures and output copyback, exceeding the live action limit",
                trigger.name
            ));
        }
        if !planned_calls.is_empty() {
            plan.calls_by_trigger
                .insert(trigger.name.clone(), planned_calls);
        }
    }
    if graph.variables.len().saturating_add(scratch_count)
        > crate::gimmicks::live_model::MAX_VARIABLES
    {
        return Err(format!(
            "move {move_name:?} routine call scratch exceeds the per-graph variable budget"
        ));
    }
    Ok(plan)
}

fn call_expanded_len(
    call: &PlannedCall,
    definitions: &BTreeMap<String, RoutineDefinition<'_>>,
) -> Result<usize, String> {
    let definition = definitions
        .get(&call.routine)
        .ok_or_else(|| format!("missing routine {:?}", call.routine))?;
    let mut count = definition
        .body
        .len()
        .checked_add(definition.outputs.len().saturating_mul(2))
        .ok_or_else(|| "expanded routine action count overflowed".to_owned())?;
    for child in &call.children {
        let child_len = child
            .inputs
            .len()
            .checked_add(call_expanded_len(child, definitions)?)
            .ok_or_else(|| "expanded routine action count overflowed".to_owned())?;
        count = count
            .checked_add(child_len)
            .ok_or_else(|| "expanded routine action count overflowed".to_owned())?;
    }
    Ok(count)
}

fn canonical_arguments<'a>(
    routine: &str,
    call: &'a RoutineCall,
    inputs: &[RoutineInput],
) -> Result<Vec<&'a RoutineArgument>, String> {
    if call.arguments.len() != inputs.len() {
        return Err(format!(
            "routine {:?} call requires {} named inputs, received {}",
            routine,
            inputs.len(),
            call.arguments.len()
        ));
    }
    let mut ordered = Vec::with_capacity(inputs.len());
    for input in inputs {
        let mut matches = call
            .arguments
            .iter()
            .filter(|argument| argument.input == input.name);
        let argument = matches.next().ok_or_else(|| {
            format!(
                "routine {:?} call is missing input {:?}",
                routine, input.name
            )
        })?;
        if matches.next().is_some() {
            return Err(format!(
                "routine {:?} call repeats input {:?}",
                routine, input.name
            ));
        }
        ordered.push(argument);
    }
    if let Some(extra) = call
        .arguments
        .iter()
        .find(|argument| !inputs.iter().any(|input| input.name == argument.input))
    {
        return Err(format!(
            "routine {:?} call supplies unknown input {:?}",
            routine, extra.input
        ));
    }
    Ok(ordered)
}

fn canonical_outputs<'a>(
    routine: &str,
    call: &'a RoutineCall,
    outputs: &[RoutineOutput],
) -> Result<Vec<&'a RoutineOutputBinding>, String> {
    if call.outputs.len() != outputs.len() {
        return Err(format!(
            "routine {:?} call requires {} named outputs, received {}",
            routine,
            outputs.len(),
            call.outputs.len()
        ));
    }
    if let Some(extra) = call
        .outputs
        .iter()
        .find(|binding| !outputs.iter().any(|output| output.name == binding.output))
    {
        return Err(format!(
            "routine {:?} supplies unknown output {:?}",
            routine, extra.output
        ));
    }
    let mut ordered = Vec::with_capacity(outputs.len());
    let mut destinations = BTreeSet::new();
    for output in outputs {
        let mut matches = call
            .outputs
            .iter()
            .filter(|binding| binding.output == output.name);
        let binding = matches.next().ok_or_else(|| {
            format!(
                "routine {:?} call is missing output {:?}",
                routine, output.name
            )
        })?;
        if matches.next().is_some() {
            return Err(format!(
                "routine {:?} repeats output {:?}",
                routine, output.name
            ));
        }
        if !destinations.insert(binding.destination.as_str()) {
            return Err(format!(
                "routine {:?} binds multiple outputs to destination {:?}",
                routine, binding.destination
            ));
        }
        ordered.push(binding);
    }
    Ok(ordered)
}

pub(super) fn expand_trigger_actions(
    graph: &MoveGraph,
    definitions: &BTreeMap<String, RoutineDefinition<'_>>,
    plan: &ExpansionPlan,
) -> Result<Vec<Vec<Action>>, String> {
    let mut expanded = Vec::with_capacity(graph.triggers.len());
    for trigger in &graph.triggers {
        let calls = graph
            .routine_calls
            .get(&trigger.name)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let planned = plan
            .calls_by_trigger
            .get(&trigger.name)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        expanded.push(expand_sequence(
            &trigger.actions,
            calls,
            planned,
            definitions,
            &BTreeMap::new(),
            &trigger.name,
        )?);
    }
    Ok(expanded)
}

fn expand_sequence(
    body: &[Action],
    calls: &[RoutineCall],
    planned: &[PlannedCall],
    definitions: &BTreeMap<String, RoutineDefinition<'_>>,
    caller_mapping: &BTreeMap<String, String>,
    context: &str,
) -> Result<Vec<Action>, String> {
    let mut output = Vec::new();
    for anchor in 0..=body.len() {
        for (index, call) in calls
            .iter()
            .enumerate()
            .filter(|(_, call)| call.before == anchor)
        {
            let call_plan = planned.get(index).ok_or_else(|| {
                format!("routine expansion plan is missing {context} call {index}")
            })?;
            output.extend(expand_call(
                call,
                call_plan,
                definitions,
                caller_mapping,
                context,
            )?);
        }
        if let Some(action) = body.get(anchor) {
            let mut projected = super::model::CustomTrigger {
                name: context.to_owned(),
                source: TriggerSource::Hook(crate::gimmicks::hooks::HookName::OnStockStart),
                guard: None,
                actions: vec![action.clone()],
            };
            references::remap_trigger_variables(&mut projected, caller_mapping);
            output.extend(projected.actions);
        }
    }
    Ok(output)
}

fn expand_call(
    call: &RoutineCall,
    planned: &PlannedCall,
    definitions: &BTreeMap<String, RoutineDefinition<'_>>,
    caller_mapping: &BTreeMap<String, String>,
    context: &str,
) -> Result<Vec<Action>, String> {
    let definition = definitions
        .get(&call.routine)
        .ok_or_else(|| format!("{context} calls missing routine {:?}", call.routine))?;
    let arguments = canonical_arguments(&call.routine, call, definition.inputs)?;
    let mut output = Vec::with_capacity(planned.inputs.len());
    for (input, argument) in planned.inputs.iter().zip(arguments) {
        let mut projected = super::model::CustomTrigger {
            name: context.to_owned(),
            source: TriggerSource::Hook(crate::gimmicks::hooks::HookName::OnStockStart),
            guard: None,
            actions: vec![match &argument.value {
                RoutineArgumentValue::Numeric(value) => Action::Set {
                    var: input.scratch.clone(),
                    value: value.clone(),
                },
                RoutineArgumentValue::Flag(value) => Action::SetFlag {
                    var: input.scratch.clone(),
                    value: value.clone(),
                },
            }],
        };
        references::remap_trigger_variables(&mut projected, caller_mapping);
        output.extend(projected.actions);
    }
    // Inputs are all captured before output slots are reset. This preserves
    // explicit input/output aliasing at the callsite.
    for output_slot in &planned.outputs {
        let action = match output_slot.kind {
            crate::gimmicks::behavior::VarType::Int => Action::Set {
                var: output_slot.scratch.clone(),
                value: crate::gimmicks::behavior::Expr::Const(
                    crate::gimmicks::behavior::ConstValue::Int(0),
                ),
            },
            crate::gimmicks::behavior::VarType::Float => Action::Set {
                var: output_slot.scratch.clone(),
                value: crate::gimmicks::behavior::Expr::Const(
                    crate::gimmicks::behavior::ConstValue::Float(0.0),
                ),
            },
            crate::gimmicks::behavior::VarType::Flag => Action::SetFlag {
                var: output_slot.scratch.clone(),
                value: crate::gimmicks::behavior::Predicate::Not(Box::new(
                    crate::gimmicks::behavior::Predicate::Compare {
                        op: crate::gimmicks::behavior::CmpOp::Eq,
                        left: crate::gimmicks::behavior::Expr::Const(
                            crate::gimmicks::behavior::ConstValue::Int(0),
                        ),
                        right: crate::gimmicks::behavior::Expr::Const(
                            crate::gimmicks::behavior::ConstValue::Int(0),
                        ),
                    },
                )),
            },
            crate::gimmicks::behavior::VarType::Timer => {
                return Err(format!(
                    "routine output {:?} cannot be a Timer",
                    output_slot.name
                ))
            }
        };
        output.push(action);
    }
    // Argument expressions are evaluated in the parent mapping above. A callee
    // body may read only its own formals; inheriting parent formals here would
    // accidentally alias same-named or undeclared values across definitions.
    let mut mapping = BTreeMap::new();
    for input in &planned.inputs {
        mapping.insert(
            references::routine_input_reference(&input.name)?,
            input.scratch.clone(),
        );
    }
    for output in &planned.outputs {
        mapping.insert(
            references::routine_output_reference(&output.name)?,
            output.scratch.clone(),
        );
    }
    let mut nested = expand_sequence(
        definition.body,
        definition.calls,
        &planned.children,
        definitions,
        &mapping,
        context,
    )?;
    output.append(&mut nested);
    for output_slot in &planned.outputs {
        let action = match output_slot.kind {
            crate::gimmicks::behavior::VarType::Int | crate::gimmicks::behavior::VarType::Float => {
                Action::Set {
                    var: output_slot.destination.clone(),
                    value: crate::gimmicks::behavior::Expr::Var(output_slot.scratch.clone()),
                }
            }
            crate::gimmicks::behavior::VarType::Flag => Action::SetFlag {
                var: output_slot.destination.clone(),
                value: crate::gimmicks::behavior::Predicate::Flag(output_slot.scratch.clone()),
            },
            crate::gimmicks::behavior::VarType::Timer => {
                return Err(format!(
                    "routine output {:?} cannot be a Timer",
                    output_slot.name
                ))
            }
        };
        let mut projected = super::model::CustomTrigger {
            name: context.to_owned(),
            source: TriggerSource::Hook(crate::gimmicks::hooks::HookName::OnStockStart),
            guard: None,
            actions: vec![action],
        };
        references::remap_trigger_variables(&mut projected, caller_mapping);
        output.extend(projected.actions);
    }
    Ok(output)
}

pub(super) fn validate_owned_routines(
    fighter: &str,
    move_name: &str,
    base: &crate::gimmicks::behavior::BehaviorGraph,
    graph: &MoveGraph,
    recipes: &BTreeMap<String, MoveGraph>,
) -> Result<(), String> {
    // Caller-only graphs may invoke a character-wide definition owned by a
    // sibling recipe, so argument bindings still need validation here.
    if graph.routines.is_empty() && graph.routine_calls.is_empty() {
        return Ok(());
    }
    use crate::gimmicks::behavior::{
        BehaviorGraph, CmpOp, ConstValue, Expr, Predicate, StateDef, Transition, VarType,
    };

    let mut vars_by_name: BTreeMap<String, crate::gimmicks::behavior::VarDef> = base
        .vars
        .iter()
        .cloned()
        .map(|var| (var.name.clone(), var))
        .collect();
    // Character-shared declarations are visible throughout the fighter's
    // trigger field. Keep one declaration here for type checking; the normal
    // trigger compiler still rejects conflicting definitions transactionally.
    for sibling in recipes.values() {
        for var in &sibling.variables {
            if sibling.scope(&var.name) == VariableScope::CharacterShared {
                vars_by_name
                    .entry(var.name.clone())
                    .or_insert_with(|| var.clone());
            }
        }
    }
    let mut mapping = BTreeMap::new();
    for var in &graph.variables {
        if graph.scope(&var.name) == VariableScope::MoveLocal {
            let runtime_name = references::runtime_variable_name(move_name, &var.name)?;
            mapping.insert(var.name.clone(), runtime_name.clone());
            let mut lowered = var.clone();
            lowered.name = runtime_name;
            vars_by_name.insert(lowered.name.clone(), lowered);
        } else {
            vars_by_name.insert(var.name.clone(), var.clone());
        }
    }

    // Preserve the character-wide event schema registry for sends inside a
    // routine, while assigning receiver ownership only to the current caller
    // transition below.
    let mut event_schemas: BTreeMap<String, Vec<crate::gimmicks::event_payload::FieldDef>> = base
        .emission_plan
        .event_bindings
        .iter()
        .map(|binding| (binding.name.clone(), binding.fields.clone()))
        .collect();
    for recipe in recipes.values() {
        for trigger in &recipe.triggers {
            if let super::model::TriggerSource::Event { name, fields } = &trigger.source {
                event_schemas
                    .entry(name.clone())
                    .or_insert_with(|| fields.clone());
            }
        }
    }

    type CallerContext = (
        Option<(String, Vec<crate::gimmicks::event_payload::FieldDef>)>,
        Option<crate::gimmicks::hooks::HookName>,
    );
    type RoutineContexts = BTreeMap<String, Vec<CallerContext>>;
    let mut contexts_by_routine = RoutineContexts::new();
    fn propagate_context(
        routine: &str,
        context: &CallerContext,
        recipes: &BTreeMap<String, MoveGraph>,
        contexts: &mut RoutineContexts,
        visited: &mut BTreeSet<String>,
        depth: usize,
    ) -> Result<(), String> {
        if depth > MAX_ROUTINE_DEPTH {
            return Err(format!(
                "routine event-context traversal exceeds depth limit {MAX_ROUTINE_DEPTH}"
            ));
        }
        let values = contexts.entry(routine.to_owned()).or_default();
        if !values.contains(context) {
            values.push(context.clone());
        }
        if !visited.insert(routine.to_owned()) {
            return Ok(());
        }
        if let Some(calls) = recipes
            .values()
            .find_map(|recipe| recipe.routine_calls_in_routines.get(routine))
        {
            for call in calls {
                propagate_context(
                    &call.routine,
                    context,
                    recipes,
                    contexts,
                    visited,
                    depth + 1,
                )?;
            }
        }
        Ok(())
    }
    for recipe in recipes.values() {
        for trigger in &recipe.triggers {
            let event_context = match &trigger.source {
                TriggerSource::Event { name, fields } => Some((name.clone(), fields.clone())),
                _ => None,
            };
            let damage_context = match trigger.source {
                TriggerSource::Hook(
                    hook @ (crate::gimmicks::hooks::HookName::OnDealDamage
                    | crate::gimmicks::hooks::HookName::OnTakeDamage),
                ) => Some(hook),
                _ => None,
            };
            let context = (event_context, damage_context);
            if let Some(calls) = recipe.routine_calls.get(&trigger.name) {
                let mut visited = BTreeSet::new();
                for call in calls {
                    propagate_context(
                        &call.routine,
                        &context,
                        recipes,
                        &mut contexts_by_routine,
                        &mut visited,
                        1,
                    )?;
                }
            }
        }
    }

    for trigger in &graph.triggers {
        let calls = graph
            .routine_calls
            .get(&trigger.name)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let owner_context = match &trigger.source {
            TriggerSource::Event { name, fields } => Some((name.clone(), fields.clone())),
            _ => None,
        };
        for (call_index, call) in calls.iter().enumerate() {
            let (_, inputs, outputs) = find_routine_definition(recipes, &call.routine)
                .ok_or_else(|| format!("call to missing routine {:?}", call.routine))?;
            let arguments = canonical_arguments(&call.routine, call, inputs)?;
            let output_bindings = canonical_outputs(&call.routine, call, outputs)?;
            let mut output_check_actions = Vec::with_capacity(outputs.len());
            let mut output_check_vars = Vec::with_capacity(outputs.len());
            for (output, binding) in outputs.iter().zip(output_bindings) {
                let destination = mapping
                    .get(&binding.destination)
                    .unwrap_or(&binding.destination);
                let target = vars_by_name.get(destination).ok_or_else(|| {
                    format!(
                        "routine {:?} output {:?} targets undeclared caller variable {:?}",
                        call.routine, output.name, binding.destination
                    )
                })?;
                if target.kind != output.kind {
                    return Err(format!(
                        "routine {:?} output {:?} requires {:?}, but destination {:?} is {:?}",
                        call.routine, output.name, output.kind, binding.destination, target.kind
                    ));
                }
                let scratch = references::routine_output_variable_name_for_path(
                    fighter,
                    move_name,
                    &trigger.name,
                    &[usize::MAX, call_index],
                    &call.routine,
                    &output.name,
                )?;
                output_check_vars.push(crate::gimmicks::behavior::VarDef {
                    keep_across_form_switch: false,
                    name: scratch.clone(),
                    kind: output.kind,
                    initial: match output.kind {
                        VarType::Int => ConstValue::Int(0),
                        VarType::Float => ConstValue::Float(0.0),
                        VarType::Flag => ConstValue::Flag(false),
                        VarType::Timer => return Err("routine output cannot be a Timer".into()),
                    },
                });
                output_check_actions.push(match output.kind {
                    VarType::Int => Action::Set {
                        var: destination.to_owned(),
                        value: Expr::Var(scratch),
                    },
                    VarType::Float => Action::Set {
                        var: destination.to_owned(),
                        value: Expr::Var(scratch),
                    },
                    VarType::Flag => Action::SetFlag {
                        var: destination.to_owned(),
                        value: Predicate::Flag(scratch),
                    },
                    VarType::Timer => return Err("routine output cannot be a Timer".into()),
                });
            }
            for (input, argument) in inputs.iter().zip(arguments) {
                let scratch = references::routine_argument_variable_name(
                    fighter,
                    move_name,
                    &trigger.name,
                    call_index,
                    &call.routine,
                    &input.name,
                )?;
                if vars_by_name.contains_key(&scratch) {
                    return Err(format!(
                        "routine {:?} input {:?} generated a colliding callsite variable",
                        call.routine, input.name
                    ));
                }
                let initial = match input.kind {
                    VarType::Int => ConstValue::Int(0),
                    VarType::Float => ConstValue::Float(0.0),
                    VarType::Flag => ConstValue::Flag(false),
                    VarType::Timer => {
                        return Err(format!("routine input {:?} cannot be a Timer", input.name))
                    }
                };
                let mut check = BehaviorGraph {
                    initial: "routine_argument_validate".into(),
                    states: vec![StateDef {
                        status: None,
                        name: "routine_argument_validate".into(),
                    }],
                    vars: vars_by_name.values().cloned().collect(),
                    ..BehaviorGraph::default()
                };
                check.vars.push(crate::gimmicks::behavior::VarDef {
                    keep_across_form_switch: false,
                    name: scratch.clone(),
                    kind: input.kind,
                    initial,
                });
                let capture_action = match &argument.value {
                    RoutineArgumentValue::Numeric(value) => {
                        crate::gimmicks::behavior::Action::Set {
                            var: scratch,
                            value: value.clone(),
                        }
                    }
                    RoutineArgumentValue::Flag(value) => {
                        crate::gimmicks::behavior::Action::SetFlag {
                            var: scratch,
                            value: value.clone(),
                        }
                    }
                };
                check.transitions.push(Transition {
                    from: "routine_argument_validate".into(),
                    to: "routine_argument_validate".into(),
                    guard: Predicate::Compare {
                        op: CmpOp::Eq,
                        left: Expr::Const(ConstValue::Int(0)),
                        right: Expr::Const(ConstValue::Int(0)),
                    },
                    on_frame: true,
                    actions: vec![capture_action],
                });
                // The expression is evaluated in the caller's scope. Apply
                // that caller's move-local mapping before checking its type;
                // the routine body receives only the captured scratch value.
                let mut caller_expression = super::model::CustomTrigger {
                    name: trigger.name.clone(),
                    source: trigger.source.clone(),
                    guard: None,
                    actions: check.transitions[0].actions.clone(),
                };
                references::remap_trigger_variables(&mut caller_expression, &mapping);
                check.transitions[0].actions = caller_expression.actions;
                if let TriggerSource::Hook(
                    hook @ (crate::gimmicks::hooks::HookName::OnDealDamage
                    | crate::gimmicks::hooks::HookName::OnTakeDamage),
                ) = trigger.source
                {
                    check
                        .emission_plan
                        .bindings
                        .push(crate::gimmicks::codegen::HookBinding {
                            hook,
                            article: None,
                            transitions: vec![0],
                        });
                }
                for (event, fields) in &event_schemas {
                    let transitions = owner_context
                        .as_ref()
                        .filter(|(owner, caller_fields)| owner == event && caller_fields == fields)
                        .map(|_| vec![0])
                        .unwrap_or_default();
                    check.emission_plan.event_bindings.push(
                        crate::gimmicks::codegen::EventBinding {
                            name: event.clone(),
                            transitions,
                            fields: fields.clone(),
                        },
                    );
                }
                if let Some(error) =
                    crate::gimmicks::behavior::validate_for_fighter(fighter, &check).first()
                {
                    return Err(format!(
                        "routine {:?} call in {}/{} input {:?}: {error}",
                        call.routine, move_name, trigger.name, input.name
                    ));
                }
            }
            if !output_check_actions.is_empty() {
                let mut check = BehaviorGraph {
                    initial: "routine_output_validate".into(),
                    states: vec![StateDef {
                        status: None,
                        name: "routine_output_validate".into(),
                    }],
                    vars: vars_by_name
                        .values()
                        .cloned()
                        .chain(output_check_vars)
                        .collect(),
                    ..BehaviorGraph::default()
                };
                check.transitions.push(Transition {
                    from: "routine_output_validate".into(),
                    to: "routine_output_validate".into(),
                    guard: Predicate::Compare {
                        op: CmpOp::Eq,
                        left: Expr::Const(ConstValue::Int(0)),
                        right: Expr::Const(ConstValue::Int(0)),
                    },
                    on_frame: true,
                    actions: output_check_actions,
                });
                for (event, fields) in &event_schemas {
                    let transitions = owner_context
                        .as_ref()
                        .filter(|(owner, caller_fields)| owner == event && caller_fields == fields)
                        .map(|_| vec![0])
                        .unwrap_or_default();
                    check.emission_plan.event_bindings.push(
                        crate::gimmicks::codegen::EventBinding {
                            name: event.clone(),
                            transitions,
                            fields: fields.clone(),
                        },
                    );
                }
                if let Some(error) =
                    crate::gimmicks::behavior::validate_for_fighter(fighter, &check).first()
                {
                    return Err(format!(
                        "routine {:?} call in {}/{} output binding: {error}",
                        call.routine, move_name, trigger.name
                    ));
                }
            }
        }
    }

    for (name, body) in &graph.routines {
        let inputs = graph
            .routine_inputs
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let outputs = graph
            .routine_outputs
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut definition_vars = vars_by_name.clone();
        for input in inputs {
            let reference = references::routine_input_reference(&input.name)?;
            if routine_body_writes(body, &reference) {
                return Err(format!(
                    "routine {name:?} writes read-only input {:?}",
                    input.name
                ));
            }
            let initial = match input.kind {
                VarType::Int => ConstValue::Int(0),
                VarType::Float => ConstValue::Float(0.0),
                VarType::Flag => ConstValue::Flag(false),
                VarType::Timer => {
                    return Err(format!("routine input {:?} cannot be a Timer", input.name))
                }
            };
            if definition_vars.contains_key(&reference) {
                return Err(format!(
                    "routine {name:?} input reference collides with a variable"
                ));
            }
            definition_vars.insert(
                reference.clone(),
                crate::gimmicks::behavior::VarDef {
                    keep_across_form_switch: false,
                    name: reference.clone(),
                    kind: input.kind,
                    initial,
                },
            );
        }
        for output in outputs {
            let reference = references::routine_output_reference(&output.name)?;
            if definition_vars.contains_key(&reference) {
                return Err(format!(
                    "routine {name:?} output reference collides with a variable"
                ));
            }
            let initial = match output.kind {
                VarType::Int => ConstValue::Int(0),
                VarType::Float => ConstValue::Float(0.0),
                VarType::Flag => ConstValue::Flag(false),
                VarType::Timer => {
                    return Err(format!(
                        "routine output {:?} cannot be a Timer",
                        output.name
                    ))
                }
            };
            definition_vars.insert(
                reference.clone(),
                crate::gimmicks::behavior::VarDef {
                    keep_across_form_switch: false,
                    name: reference,
                    kind: output.kind,
                    initial,
                },
            );
        }
        let readonly_input_references: BTreeSet<String> = inputs
            .iter()
            .map(|input| references::routine_input_reference(&input.name))
            .collect::<Result<_, _>>()?;
        let nested_calls = graph
            .routine_calls_in_routines
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut nested_capture_actions = Vec::new();
        for (call_index, call) in nested_calls.iter().enumerate() {
            let (_, callee_inputs, callee_outputs) =
                find_routine_definition(recipes, &call.routine).ok_or_else(|| {
                    format!("routine {name:?} calls missing routine {:?}", call.routine)
                })?;
            let arguments = canonical_arguments(&call.routine, call, callee_inputs)?;
            for (input_index, (input, argument)) in callee_inputs.iter().zip(arguments).enumerate()
            {
                let scratch = references::routine_argument_variable_name_for_path(
                    fighter,
                    move_name,
                    "routine",
                    &[usize::MAX, call_index],
                    &call.routine,
                    &input.name,
                )?;
                if definition_vars.contains_key(&scratch) {
                    return Err(format!(
                        "nested routine capture for {name:?} collides with a variable"
                    ));
                }
                let initial = match input.kind {
                    VarType::Int => ConstValue::Int(0),
                    VarType::Float => ConstValue::Float(0.0),
                    VarType::Flag => ConstValue::Flag(false),
                    VarType::Timer => {
                        return Err(format!("routine input {:?} cannot be a Timer", input.name))
                    }
                };
                definition_vars.insert(
                    scratch.clone(),
                    crate::gimmicks::behavior::VarDef {
                        keep_across_form_switch: false,
                        name: scratch.clone(),
                        kind: input.kind,
                        initial,
                    },
                );
                let capture = match &argument.value {
                    RoutineArgumentValue::Numeric(value) => {
                        crate::gimmicks::behavior::Action::Set {
                            var: scratch,
                            value: value.clone(),
                        }
                    }
                    RoutineArgumentValue::Flag(value) => {
                        crate::gimmicks::behavior::Action::SetFlag {
                            var: scratch,
                            value: value.clone(),
                        }
                    }
                };
                let _ = (input_index, call_index);
                nested_capture_actions.push(capture);
            }
            let bindings = canonical_outputs(&call.routine, call, callee_outputs)?;
            for (output, binding) in callee_outputs.iter().zip(bindings) {
                let scratch = references::routine_output_variable_name_for_path(
                    fighter,
                    move_name,
                    "routine",
                    &[usize::MAX, call_index],
                    &call.routine,
                    &output.name,
                )?;
                if definition_vars.contains_key(&scratch) {
                    return Err(format!(
                        "nested routine output for {name:?} collides with a variable"
                    ));
                }
                let initial = match output.kind {
                    VarType::Int => ConstValue::Int(0),
                    VarType::Float => ConstValue::Float(0.0),
                    VarType::Flag => ConstValue::Flag(false),
                    VarType::Timer => return Err("routine output cannot be a Timer".into()),
                };
                definition_vars.insert(
                    scratch.clone(),
                    crate::gimmicks::behavior::VarDef {
                        keep_across_form_switch: false,
                        name: scratch.clone(),
                        kind: output.kind,
                        initial,
                    },
                );
                let destination = mapping
                    .get(&binding.destination)
                    .unwrap_or(&binding.destination);
                if readonly_input_references.contains(destination) {
                    return Err(format!(
                        "nested routine {name:?} cannot bind output {:?} to read-only input {:?}",
                        output.name, binding.destination
                    ));
                }
                let target_kind = definition_vars.get(destination).map(|target| target.kind);
                if target_kind != Some(output.kind) {
                    return Err(format!(
                        "nested routine {name:?} output {:?} targets undeclared or wrong-kind variable {:?}",
                        output.name, binding.destination
                    ));
                }
                nested_capture_actions.push(match output.kind {
                    VarType::Int | VarType::Float => Action::Set {
                        var: destination.to_owned(),
                        value: Expr::Var(scratch),
                    },
                    VarType::Flag => Action::SetFlag {
                        var: destination.to_owned(),
                        value: Predicate::Flag(scratch),
                    },
                    VarType::Timer => return Err("routine output cannot be a Timer".into()),
                });
            }
        }
        // EventField/EventFlag permissions belong to the actual caller, not
        // to the routine definition. Check each distinct source context that
        // calls this body. Uncalled bodies get one fieldless context so a
        // saved scoped read cannot hide until a future call is authored.
        let mut contexts = contexts_by_routine.get(name).cloned().unwrap_or_default();
        if contexts.is_empty() {
            contexts.push((None, None));
        }

        for context in contexts {
            let mut lowered = super::model::CustomTrigger {
                name: name.clone(),
                // Only actions are validated in this projection; the exact
                // receiver context is installed on synthetic transition 0.
                source: super::model::TriggerSource::Hook(
                    crate::gimmicks::hooks::HookName::OnStockStart,
                ),
                guard: None,
                actions: body
                    .iter()
                    .cloned()
                    .chain(nested_capture_actions.iter().cloned())
                    .collect(),
            };
            references::remap_trigger_variables(&mut lowered, &mapping);
            let mut check = BehaviorGraph {
                initial: "routine_validate".into(),
                states: vec![StateDef {
                    status: None,
                    name: "routine_validate".into(),
                }],
                vars: definition_vars.values().cloned().collect(),
                ..BehaviorGraph::default()
            };
            check.transitions.push(Transition {
                from: "routine_validate".into(),
                to: "routine_validate".into(),
                guard: Predicate::Compare {
                    op: CmpOp::Eq,
                    left: Expr::Const(ConstValue::Int(0)),
                    right: Expr::Const(ConstValue::Int(0)),
                },
                on_frame: true,
                actions: lowered.actions,
            });
            for (event, fields) in &event_schemas {
                let transitions = context
                    .0
                    .as_ref()
                    .filter(|(owner, caller_fields)| owner == event && caller_fields == fields)
                    .map(|_| vec![0])
                    .unwrap_or_default();
                check
                    .emission_plan
                    .event_bindings
                    .push(crate::gimmicks::codegen::EventBinding {
                        name: event.clone(),
                        transitions,
                        fields: fields.clone(),
                    });
            }
            if let Some(hook) = context.1 {
                check
                    .emission_plan
                    .bindings
                    .push(crate::gimmicks::codegen::HookBinding {
                        hook,
                        article: None,
                        transitions: vec![0],
                    });
            }
            // This is a type-checking projection, not an installed invocation.
            // Unrelated character declarations and unused result ports
            // must not consume its temporary table budget. Real expanded
            // invocation storage is bounded separately for the whole character.
            let referenced = references::behavior_referenced_variables(&check);
            check.vars.retain(|var| {
                referenced.contains(&var.name)
                // Rename/reference walkers intentionally exclude numeric
                // input formals, which are not caller-owned declarations.
                || readonly_input_references.contains(&var.name)
            });
            if let Some(error) =
                crate::gimmicks::behavior::validate_for_fighter(fighter, &check).first()
            {
                let caller = context.0.as_ref().map_or_else(
                    || "a caller without incoming event fields".to_owned(),
                    |(event, _)| format!("event {event:?}"),
                );
                return Err(format!(
                    "invalid routine body {name:?} for {caller}: {error}"
                ));
            }
        }
    }
    Ok(())
}

fn routine_body_writes(actions: &[Action], placeholder: &str) -> bool {
    actions.iter().any(|action| match action {
        Action::Set { var, .. }
        | Action::SetFlag { var, .. }
        | Action::Add { var, .. }
        | Action::Raise { var }
        | Action::Clear { var }
        | Action::Reset { var } => var == placeholder,
        _ => false,
    })
}
pub(super) fn find_routine_definition<'a>(
    recipes: &'a BTreeMap<String, MoveGraph>,
    routine: &str,
) -> Option<RoutineParts<'a>> {
    recipes.values().find_map(|graph| {
        graph.routines.get(routine).map(|body| {
            let inputs = graph
                .routine_inputs
                .get(routine)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let outputs = graph
                .routine_outputs
                .get(routine)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            (body.as_slice(), inputs, outputs)
        })
    })
}

type RoutineParts<'a> = (&'a [Action], &'a [RoutineInput], &'a [RoutineOutput]);
