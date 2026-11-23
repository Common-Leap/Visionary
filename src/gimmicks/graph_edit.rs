//! Atomic, typed edits for authored behavior graphs.
//!
//! The advanced graph editor may still expose raw JSON for repair, but routine
//! selector-driven edits should go through this module. Every operation works
//! on a clone and returns it only when the resulting graph is valid for the
//! selected fighter. Existing reviewed status/resource sources are preserved;
//! these APIs never create a new source or accept engine identifiers as raw
//! numbers.

use super::behavior::{
    self, Action, BehaviorGraph, ConstValue, Expr, Predicate, StateDef, Transition, VarDef,
};
use super::codegen::{self, HookBinding, StatusTrigger};
use super::event_payload::EventValue;
use super::hooks::{self, HookName, SupportState};
use super::native_inputs::CombatOp;

/// Return the hook choices that can be bound without a fighter-specific proof.
pub fn direct_hooks() -> Vec<HookName> {
    hooks::all()
        .iter()
        .filter_map(|definition| {
            matches!(definition.state, SupportState::Supported { .. }).then_some(definition.hook)
        })
        .collect()
}

/// Validate structural, fighter-scoped, live-runtime, hook-plan, and emitted
/// source constraints. A graph with no emission plan is an editable draft; as
/// soon as it has bindings, the complete plan and generated source must pass.
pub fn validate_graph(graph: &BehaviorGraph, fighter: &str) -> Result<(), String> {
    if fighter.trim().is_empty() {
        return Err("choose a fighter before editing a behavior graph".into());
    }
    if super::registry::fighter_kind_symbol(fighter).is_none() {
        return Err(format!(
            "fighter {fighter:?} has no exact pinned fighter-kind mapping"
        ));
    }
    validate_model_bounds(graph)?;
    let problems = behavior::validate_for_fighter(fighter, graph);
    if !problems.is_empty() {
        return Err(problems
            .into_iter()
            .map(|problem| problem.to_string())
            .collect::<Vec<_>>()
            .join("; "));
    }
    if graph.is_empty()
        || (graph.emission_plan.is_empty() && graph.states.iter().all(|s| s.status.is_none()))
    {
        return Ok(());
    }
    let plan_problems = codegen::check_plan(graph, &graph.emission_plan);
    if !plan_problems.is_empty() {
        return Err(plan_problems
            .into_iter()
            .map(|problem| problem.to_string())
            .collect::<Vec<_>>()
            .join("; "));
    }
    let source = super::verifier::verify_behavior(fighter, graph, &graph.emission_plan);
    if !source.is_clean() {
        return Err(source.blocker_summary().join("; "));
    }
    super::live_protocol::program_from_graph(fighter, graph).map(|_| ())
}

/// Non-blocking warnings: `(later, earlier)` frame rules where the later one
/// can never run. Every passing rule runs, in binding order, self-loops
/// included; a rule that moves to a plain state ends the walk for its
/// siblings (a status-bound target only requests the status); so a later rule from the
/// same state is dead when an earlier one that leaves passes whenever it does
/// (no guard, the same guard, or a guard made of a subset of its conditions).
pub fn unreachable_frame_rules(graph: &BehaviorGraph) -> Vec<(usize, usize)> {
    fn conditions<'a>(guard: &'a Predicate, out: &mut Vec<&'a Predicate>) {
        match guard {
            Predicate::And(a, b) => {
                conditions(a, out);
                conditions(b, out);
            }
            other if *other == super::templates::always() => {}
            other => out.push(other),
        }
    }
    let mut found = Vec::new();
    for binding in &graph.emission_plan.bindings {
        let rules: Vec<_> = binding
            .transitions
            .iter()
            .filter_map(|i| {
                graph
                    .transitions
                    .get(*i)
                    .filter(|t| t.on_frame)
                    .map(|t| (*i, t))
            })
            .collect();
        for (n, (later, rule)) in rules.iter().enumerate() {
            let mut own = Vec::new();
            conditions(&rule.guard, &mut own);
            let shadow = rules[..n].iter().find(|(_, earlier)| {
                let mut theirs = Vec::new();
                conditions(&earlier.guard, &mut theirs);
                // A status-bound target only requests the status; the walk stays in `from`.
                let leaves = earlier.to != earlier.from
                    && graph
                        .states
                        .iter()
                        .any(|s| s.name == earlier.to && s.status.is_none());
                earlier.from == rule.from && leaves && theirs.iter().all(|c| own.contains(c))
            });
            if let Some((earlier, _)) = shadow {
                if !found.iter().any(|(l, _)| l == later) {
                    found.push((*later, *earlier));
                }
            }
        }
    }
    found
}

/// Set the entry state to an existing state.
pub fn set_initial(
    graph: &BehaviorGraph,
    fighter: &str,
    name: &str,
) -> Result<BehaviorGraph, String> {
    let mut updated = graph.clone();
    if !updated.states.iter().any(|state| state.name == name) {
        return Err(format!("entry state {name:?} does not exist"));
    }
    updated.initial = name.to_owned();
    commit(updated, fighter)
}

/// Add a uniquely named state. The first state becomes the graph entry.
pub fn add_state(
    graph: &BehaviorGraph,
    fighter: &str,
    name: impl Into<String>,
) -> Result<BehaviorGraph, String> {
    let name = name.into();
    check_new_name(&name, "state")?;
    if graph.states.iter().any(|state| state.name == name) {
        return Err(format!("state {name:?} already exists"));
    }
    let mut updated = graph.clone();
    if updated.initial.is_empty() {
        updated.initial = name.clone();
    }
    updated.states.push(StateDef { name, status: None });
    commit(updated, fighter)
}

/// Rename a state and update the entry and every transition endpoint.
pub fn rename_state(
    graph: &BehaviorGraph,
    fighter: &str,
    old: &str,
    new: impl Into<String>,
) -> Result<BehaviorGraph, String> {
    let new = new.into();
    check_new_name(&new, "state")?;
    if graph.initial != old && graph.states.iter().all(|state| state.name != old) {
        return Err(format!("state {old:?} does not exist"));
    }
    if graph.states.iter().any(|state| state.name == new) {
        return Err(format!("state {new:?} already exists"));
    }
    let mut updated = graph.clone();
    let state = updated
        .states
        .iter_mut()
        .find(|state| state.name == old)
        .ok_or_else(|| format!("state {old:?} does not exist"))?;
    state.name.clone_from(&new);
    if updated.initial == old {
        updated.initial.clone_from(&new);
    }
    for transition in &mut updated.transitions {
        if transition.from == old {
            transition.from.clone_from(&new);
        }
        if transition.to == old {
            transition.to.clone_from(&new);
        }
    }
    commit(updated, fighter)
}

fn status_roots(graph: &mut BehaviorGraph) -> impl Iterator<Item = &mut Vec<usize>> {
    graph
        .states
        .iter_mut()
        .filter_map(|state| state.status.as_mut())
        .flat_map(|binding| binding.roots.values_mut())
}

/// Let a non-entry state own an existing fighter status, or release it. Owning adds the
/// three rules every owned status needs: initialize, play a motion, and leave when it ends.
#[cfg(test)]
pub fn set_state_status(
    graph: &BehaviorGraph,
    fighter: &str,
    state: &str,
    target: Option<&str>,
) -> Result<BehaviorGraph, String> {
    set_state_status_ref(graph, fighter, state, target.map(Into::into))
}
pub fn set_state_status_ref(
    graph: &BehaviorGraph,
    fighter: &str,
    state: &str,
    target: Option<super::status_model::StatusRef>,
) -> Result<BehaviorGraph, String> {
    use super::status_commands::{Command, MotionFrame};
    use super::status_model::{Phase, Policy, StatusBinding};
    if graph.initial == state {
        return Err("the entry state cannot own a fighter status".into());
    }
    let index = graph
        .states
        .iter()
        .position(|candidate| candidate.name == state)
        .ok_or_else(|| format!("state {state:?} does not exist"))?;
    let mut updated = graph.clone();
    if let Some(super::status_model::StatusRef::Custom { key, slot }) = &target {
        if updated
            .custom_statuses
            .get(key)
            .is_some_and(|old| old != slot)
        {
            return Err("custom status key already has another slot".into());
        }
        updated.custom_statuses.insert(key.clone(), *slot);
    }
    match (target, updated.states[index].status.take()) {
        (Some(target), Some(mut binding)) => {
            binding.target = target;
            updated.states[index].status = Some(binding);
        }
        (Some(target), None) => {
            let always = super::templates::always();
            // A status symbol is not an animation name. Use a known common
            // animation until an animation is selected explicitly.
            let motion = "wait".to_owned();
            let rule = |to: &str, guard: Predicate, action: Command<Expr>| Transition {
                from: state.into(),
                to: to.into(),
                guard,
                actions: vec![Action::Status(action)],
                on_frame: false,
            };
            let first = updated.transitions.len();
            updated.transitions.extend([
                rule(
                    state,
                    always.clone(),
                    Command::InitializeStatus(Default::default()),
                ),
                rule(
                    state,
                    always,
                    Command::PlayMotion {
                        motion,
                        frame: MotionFrame::Start(Expr::Const(ConstValue::Float(0.0))),
                        rate: Expr::Const(ConstValue::Float(1.0)),
                    },
                ),
            ]);
            updated.states[index].status = Some(StatusBinding {
                target,
                policy: Policy::Owned,
                roots: [
                    (Phase::Pre, vec![first]),
                    (Phase::MainEnter, vec![first + 1]),
                    (Phase::MainLoop, vec![]),
                ]
                .into(),
            });
            updated = super::status_recipes::finish(&updated, fighter, state, true)?;
        }
        (None, Some(binding)) => {
            let mut owned: Vec<usize> = binding.roots.values().flatten().copied().collect();
            owned.sort_unstable();
            for index in owned.into_iter().rev() {
                remove_transition_at(&mut updated, index);
            }
        }
        (None, None) => return Ok(updated),
    }
    commit(updated, fighter)
}

/// Remove an unreferenced, non-entry state.
pub fn remove_state(
    graph: &BehaviorGraph,
    fighter: &str,
    name: &str,
) -> Result<BehaviorGraph, String> {
    if graph.initial == name {
        return Err(format!("cannot remove entry state {name:?}"));
    }
    if graph
        .transitions
        .iter()
        .any(|transition| transition.from == name || transition.to == name)
    {
        return Err(format!(
            "cannot remove state {name:?}; a transition references it"
        ));
    }
    let mut updated = graph.clone();
    let before = updated.states.len();
    updated.states.retain(|state| state.name != name);
    if before == updated.states.len() {
        return Err(format!("state {name:?} does not exist"));
    }
    commit(updated, fighter)
}

/// Add a uniquely named typed variable.
pub fn add_variable(
    graph: &BehaviorGraph,
    fighter: &str,
    variable: VarDef,
) -> Result<BehaviorGraph, String> {
    check_new_name(&variable.name, "variable")?;
    if graph
        .vars
        .iter()
        .any(|existing| existing.name == variable.name)
    {
        return Err(format!("variable {:?} already exists", variable.name));
    }
    let mut updated = graph.clone();
    updated.vars.push(variable);
    commit(updated, fighter)
}

/// Rename a variable and rewrite every expression, guard, and action reference.
pub fn rename_variable(
    graph: &BehaviorGraph,
    fighter: &str,
    old: &str,
    new: impl Into<String>,
) -> Result<BehaviorGraph, String> {
    validate_model_bounds(graph)?;
    let new = new.into();
    check_new_name(&new, "variable")?;
    if !graph.vars.iter().any(|variable| variable.name == old) {
        return Err(format!("variable {old:?} does not exist"));
    }
    if graph.vars.iter().any(|variable| variable.name == new) {
        return Err(format!("variable {new:?} already exists"));
    }
    let mut updated = graph.clone();
    let variable = updated
        .vars
        .iter_mut()
        .find(|variable| variable.name == old)
        .expect("checked above");
    variable.name.clone_from(&new);
    for transition in &mut updated.transitions {
        rewrite_predicate(&mut transition.guard, old, &new);
        for action in &mut transition.actions {
            rewrite_action(action, old, &new);
        }
    }
    commit(updated, fighter)
}

/// Remove a variable only when no guard or action refers to it.
pub fn remove_variable(
    graph: &BehaviorGraph,
    fighter: &str,
    name: &str,
) -> Result<BehaviorGraph, String> {
    validate_model_bounds(graph)?;
    if !graph.vars.iter().any(|variable| variable.name == name) {
        return Err(format!("variable {name:?} does not exist"));
    }
    if graph.transitions.iter().any(|transition| {
        predicate_references(&transition.guard, name)
            || transition
                .actions
                .iter()
                .any(|action| action_references(action, name))
    }) {
        return Err(format!(
            "cannot remove variable {name:?}; a guard or action uses it"
        ));
    }
    let mut updated = graph.clone();
    updated.vars.retain(|variable| variable.name != name);
    commit(updated, fighter)
}

/// Add a transition and bind it atomically to one directly supported hook.
pub fn add_transition(
    graph: &BehaviorGraph,
    fighter: &str,
    transition: Transition,
    hook: HookName,
) -> Result<BehaviorGraph, String> {
    ensure_hook_is_not_source_bound(graph, hook)?;
    require_direct_hook(hook)?;
    let mut updated = graph.clone();
    let index = updated.transitions.len();
    updated.transitions.push(transition);
    if let Some(binding) = updated
        .emission_plan
        .bindings
        .iter_mut()
        .find(|binding| binding.hook == hook)
    {
        binding.transitions.push(index);
    } else {
        updated.emission_plan.bindings.push(HookBinding {
            hook,
            article: None,
            transitions: vec![index],
        });
    }
    commit(updated, fighter)
}

/// Add one transition run when a move sends this controller the named event,
/// creating the receiver if it does not exist yet.
pub fn add_event_transition(
    graph: &BehaviorGraph,
    fighter: &str,
    transition: Transition,
    name: &str,
) -> Result<BehaviorGraph, String> {
    let mut updated = graph.clone();
    let index = updated.transitions.len();
    updated.transitions.push(transition);
    let bindings = &mut updated.emission_plan.event_bindings;
    if let Some(binding) = bindings.iter_mut().find(|binding| binding.name == name) {
        binding.transitions.push(index);
    } else {
        bindings.push(super::codegen::EventBinding {
            name: name.into(),
            transitions: vec![index],
            fields: Vec::new(),
        });
    }
    commit(updated, fighter)
}

/// Rename one event receiver. Moves that send the old name must be rewired.
pub fn rename_event(
    graph: &BehaviorGraph,
    fighter: &str,
    index: usize,
    name: &str,
) -> Result<BehaviorGraph, String> {
    if name.is_empty() {
        return Err("event names cannot be empty".into());
    }
    if graph
        .emission_plan
        .event_bindings
        .iter()
        .any(|binding| binding.name == name)
    {
        return Err(format!("this controller already receives {name:?}"));
    }
    let mut updated = graph.clone();
    updated
        .emission_plan
        .event_bindings
        .get_mut(index)
        .ok_or("event receiver no longer exists")?
        .name = name.into();
    commit(updated, fighter)
}

/// Add one exact status-edge transition without disturbing other hook,
/// resource, or reviewed status bindings. The status symbol is the status
/// being entered or exited according to `hook`.
pub fn add_status_transition(
    graph: &BehaviorGraph,
    fighter: &str,
    transition: Transition,
    hook: HookName,
    symbol: &str,
) -> Result<BehaviorGraph, String> {
    if !matches!(hook, HookName::OnStatusEnter | HookName::OnStatusExit) {
        return Err(format!("{} is not an exact status-edge hook", hook.name()));
    }
    if !transition.on_frame {
        return Err("status-edge transitions must declare an event boundary".into());
    }
    super::registry::resolve(
        fighter,
        symbol,
        super::registry::IdentifierKind::FighterStatus,
    )
    .map_err(|error| format!("status symbol {symbol:?}: {error}"))?;

    let mut updated = graph.clone();
    let index = updated.transitions.len();
    updated.transitions.push(transition);
    if let Some(binding) = updated
        .emission_plan
        .bindings
        .iter_mut()
        .find(|binding| binding.hook == hook)
    {
        binding.transitions.push(index);
    } else {
        updated.emission_plan.bindings.push(HookBinding {
            hook,
            article: None,
            transitions: vec![index],
        });
    }
    if let Some(trigger) = updated
        .emission_plan
        .status_triggers
        .iter_mut()
        .find(|trigger| trigger.hook == hook && trigger.symbol == symbol)
    {
        trigger.transitions.push(index);
    } else {
        updated.emission_plan.status_triggers.push(StatusTrigger {
            hook,
            symbol: symbol.to_owned(),
            evidence: String::new(),
            runtime_validated: false,
            transitions: vec![index],
        });
    }
    commit(updated, fighter)
}

/// Append an action to one transition and validate the complete resulting graph.
pub fn add_action(
    graph: &BehaviorGraph,
    fighter: &str,
    transition_index: usize,
    action: Action,
) -> Result<BehaviorGraph, String> {
    let mut updated = graph.clone();
    let transition_count = updated.transitions.len();
    let transition = updated.transitions.get_mut(transition_index).ok_or_else(|| {
        format!(
            "cannot add action to transition {transition_index}: transition index is out of range for {} transitions",
            transition_count
        )
    })?;
    let action_index = transition.actions.len();
    transition.actions.push(action);
    commit(updated, fighter).map_err(|error| {
        format!("cannot add action {action_index} to transition {transition_index}: {error}")
    })
}

/// Replace one action and validate the complete resulting graph.
pub fn replace_action(
    graph: &BehaviorGraph,
    fighter: &str,
    transition_index: usize,
    action_index: usize,
    action: Action,
) -> Result<BehaviorGraph, String> {
    let mut updated = graph.clone();
    let transition_count = updated.transitions.len();
    let transition = updated.transitions.get_mut(transition_index).ok_or_else(|| {
        format!(
            "cannot replace action {action_index} in transition {transition_index}: transition index is out of range for {} transitions",
            transition_count
        )
    })?;
    let action_count = transition.actions.len();
    let target = transition.actions.get_mut(action_index).ok_or_else(|| {
        format!(
            "cannot replace action {action_index} in transition {transition_index}: action index is out of range for {action_count} actions"
        )
    })?;
    *target = action;
    commit(updated, fighter).map_err(|error| {
        format!("cannot replace action {action_index} in transition {transition_index}: {error}")
    })
}

/// Remove one action and validate the complete resulting graph.
pub fn remove_action(
    graph: &BehaviorGraph,
    fighter: &str,
    transition_index: usize,
    action_index: usize,
) -> Result<BehaviorGraph, String> {
    let mut updated = graph.clone();
    let transition_count = updated.transitions.len();
    let transition = updated.transitions.get_mut(transition_index).ok_or_else(|| {
        format!(
            "cannot remove action {action_index} from transition {transition_index}: transition index is out of range for {} transitions",
            transition_count
        )
    })?;
    let action_count = transition.actions.len();
    if action_index >= action_count {
        return Err(format!(
            "cannot remove action {action_index} from transition {transition_index}: action index is out of range for {action_count} actions"
        ));
    }
    transition.actions.remove(action_index);
    commit(updated, fighter).map_err(|error| {
        format!("cannot remove action {action_index} from transition {transition_index}: {error}")
    })
}

/// Move one action within its transition and validate the resulting graph.
pub fn reorder_action(
    graph: &BehaviorGraph,
    fighter: &str,
    transition_index: usize,
    from: usize,
    to: usize,
) -> Result<BehaviorGraph, String> {
    let mut updated = graph.clone();
    let transition_count = updated.transitions.len();
    let transition = updated.transitions.get_mut(transition_index).ok_or_else(|| {
        format!(
            "cannot move action {from} -> {to} in transition {transition_index}: transition index is out of range for {} transitions",
            transition_count
        )
    })?;
    let count = transition.actions.len();
    if from >= count || to >= count {
        return Err(format!(
            "cannot move action {from} -> {to} in transition {transition_index}: action indices must be within {count} actions"
        ));
    }
    let action = transition.actions.remove(from);
    transition.actions.insert(to, action);
    commit(updated, fighter).map_err(|error| {
        format!("cannot move action {from} -> {to} in transition {transition_index}: {error}")
    })
}

/// Replace one transition without changing its hook ownership.
pub fn replace_transition(
    graph: &BehaviorGraph,
    fighter: &str,
    index: usize,
    transition: Transition,
) -> Result<BehaviorGraph, String> {
    let mut updated = graph.clone();
    let target = updated
        .transitions
        .get_mut(index)
        .ok_or_else(|| format!("transition index {index} is out of range"))?;
    *target = transition;
    commit(updated, fighter)
}

/// Delete a transition, shift all later binding indices, and remove any hook
/// binding and reviewed trigger that no longer owns a transition.
pub fn delete_transition(
    graph: &BehaviorGraph,
    fighter: &str,
    index: usize,
) -> Result<BehaviorGraph, String> {
    let mut updated = graph.clone();
    if index >= updated.transitions.len() {
        return Err(format!("transition index {index} is out of range"));
    }
    remove_transition_at(&mut updated, index);
    commit(updated, fighter)
}

pub(super) fn remove_transition_at(updated: &mut BehaviorGraph, index: usize) {
    updated.transitions.remove(index);
    for transitions in status_roots(updated) {
        transitions.retain(|&bound| bound != index);
        transitions
            .iter_mut()
            .for_each(|bound| *bound -= usize::from(*bound > index));
    }
    for transitions in updated
        .emission_plan
        .bindings
        .iter_mut()
        .map(|binding| &mut binding.transitions)
        .chain(
            updated
                .emission_plan
                .event_bindings
                .iter_mut()
                .map(|binding| &mut binding.transitions),
        )
    {
        *transitions = transitions
            .iter()
            .filter_map(|&bound| {
                if bound == index {
                    None
                } else if bound > index {
                    Some(bound - 1)
                } else {
                    Some(bound)
                }
            })
            .collect();
    }
    updated
        .emission_plan
        .event_bindings
        .retain(|binding| !binding.transitions.is_empty());
    let emptied_hooks = updated
        .emission_plan
        .bindings
        .iter()
        .filter(|binding| binding.transitions.is_empty())
        .map(|binding| binding.hook)
        .collect::<Vec<_>>();
    updated
        .emission_plan
        .bindings
        .retain(|binding| !binding.transitions.is_empty());
    updated
        .emission_plan
        .status_triggers
        .retain(|trigger| !emptied_hooks.contains(&trigger.hook));
    updated
        .emission_plan
        .resource_triggers
        .retain(|trigger| !emptied_hooks.contains(&trigger.hook));
}

/// Move a transition while preserving each hook's authored transition order.
pub fn reorder_transition(
    graph: &BehaviorGraph,
    fighter: &str,
    from: usize,
    to: usize,
) -> Result<BehaviorGraph, String> {
    let mut updated = graph.clone();
    let count = updated.transitions.len();
    if from >= count || to >= count {
        return Err(format!(
            "transition move {from} -> {to} is out of range for {count} transitions"
        ));
    }
    let transition = updated.transitions.remove(from);
    updated.transitions.insert(to, transition);
    for transitions in status_roots(&mut updated) {
        transitions
            .iter_mut()
            .for_each(|index| *index = remap_move(*index, from, to));
    }
    for transitions in updated
        .emission_plan
        .bindings
        .iter_mut()
        .map(|binding| &mut binding.transitions)
        .chain(
            updated
                .emission_plan
                .event_bindings
                .iter_mut()
                .map(|binding| &mut binding.transitions),
        )
    {
        for index in transitions {
            *index = remap_move(*index, from, to);
        }
    }
    commit(updated, fighter)
}

fn commit(graph: BehaviorGraph, fighter: &str) -> Result<BehaviorGraph, String> {
    validate_graph(&graph, fighter)?;
    Ok(graph)
}

fn require_direct_hook(hook: HookName) -> Result<(), String> {
    match hooks::lookup(hook).state {
        SupportState::Supported { .. } => Ok(()),
        SupportState::StatusBound { .. } => Err(format!(
            "{} requires an existing reviewed status or resource proof",
            hook.name()
        )),
        SupportState::Unavailable { reason, .. } => {
            Err(format!("{} is unavailable: {reason}", hook.name()))
        }
    }
}

fn ensure_hook_is_not_source_bound(graph: &BehaviorGraph, hook: HookName) -> Result<(), String> {
    if graph
        .emission_plan
        .status_triggers
        .iter()
        .any(|trigger| trigger.hook == hook)
        || graph
            .emission_plan
            .resource_triggers
            .iter()
            .any(|trigger| trigger.hook == hook)
    {
        return Err(format!(
            "{} already has a reviewed status or native resource source; edit that source binding through its dedicated controls",
            hook.name()
        ));
    }
    Ok(())
}

fn check_new_name(name: &str, category: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        Err(format!("{category} name cannot be empty"))
    } else if name.len() > super::live_model::MAX_NAME_BYTES {
        Err(format!(
            "{category} name exceeds {} bytes",
            super::live_model::MAX_NAME_BYTES
        ))
    } else {
        Ok(())
    }
}

fn validate_model_bounds(graph: &BehaviorGraph) -> Result<(), String> {
    use super::live_model::{
        MAX_ABS_NUMBER, MAX_ACTIONS_PER_TRANSITION, MAX_BINDINGS, MAX_EXPR_DEPTH, MAX_EXPR_NODES,
        MAX_NAME_BYTES, MAX_STATES, MAX_SYMBOL_BYTES, MAX_TIMER_FRAMES, MAX_TRANSITIONS,
        MAX_VARIABLES,
    };

    let mut errors = Vec::new();
    if graph.states.len() > MAX_STATES {
        errors.push(format!(
            "graph has {} states; runtime limit is {MAX_STATES}",
            graph.states.len()
        ));
    }
    if graph.vars.len() > MAX_VARIABLES {
        errors.push(format!(
            "graph has {} variables; runtime limit is {MAX_VARIABLES}",
            graph.vars.len()
        ));
    }
    if graph.transitions.len() > MAX_TRANSITIONS {
        errors.push(format!(
            "graph has {} transitions; runtime limit is {MAX_TRANSITIONS}",
            graph.transitions.len()
        ));
    }
    if graph.emission_plan.bindings.len() > MAX_BINDINGS
        || graph.emission_plan.event_bindings.len() > MAX_BINDINGS
        || graph.emission_plan.status_triggers.len() > MAX_BINDINGS
        || graph.emission_plan.resource_triggers.len() > MAX_BINDINGS
    {
        errors.push(format!(
            "hook bindings and each trigger list are limited to {MAX_BINDINGS}"
        ));
    }
    check_text_bound(&graph.initial, MAX_NAME_BYTES, "entry state", &mut errors);
    for event in &graph.emission_plan.event_bindings {
        check_text_bound(&event.name, MAX_NAME_BYTES, "event name", &mut errors);
    }
    for state in &graph.states {
        check_text_bound(&state.name, MAX_NAME_BYTES, "state name", &mut errors);
    }
    for variable in &graph.vars {
        check_text_bound(&variable.name, MAX_NAME_BYTES, "variable name", &mut errors);
        match variable.initial {
            ConstValue::Float(value)
                if !value.is_finite() || f64::from(value).abs() > MAX_ABS_NUMBER =>
            {
                errors.push(format!(
                    "variable {:?} has a non-finite or out-of-range initial value",
                    variable.name
                ));
            }
            ConstValue::Int(value) if i64::from(value).abs() > MAX_ABS_NUMBER as i64 => {
                errors.push(format!(
                    "variable {:?} has an out-of-range initial value",
                    variable.name
                ));
            }
            _ => {}
        }
        if variable.kind == behavior::VarType::Timer
            && matches!(variable.initial, ConstValue::Int(value) if !(0..=MAX_TIMER_FRAMES).contains(&value))
        {
            errors.push(format!(
                "timer {:?} must start between zero and {MAX_TIMER_FRAMES}",
                variable.name
            ));
        }
    }
    let mut total_expression_nodes = 0usize;
    for (index, transition) in graph.transitions.iter().enumerate() {
        check_text_bound(
            &transition.from,
            MAX_NAME_BYTES,
            "transition source",
            &mut errors,
        );
        check_text_bound(
            &transition.to,
            MAX_NAME_BYTES,
            "transition target",
            &mut errors,
        );
        check_predicate_text(&transition.guard, 1, &mut errors);
        if transition.actions.len() > MAX_ACTIONS_PER_TRANSITION {
            errors.push(format!(
                "transition {index} has {} actions; runtime limit is {MAX_ACTIONS_PER_TRANSITION}",
                transition.actions.len()
            ));
        }
        let (mut nodes, mut depth) = predicate_size(&transition.guard, 1, &mut errors, index);
        for action in &transition.actions {
            check_action_text(action, &mut errors);
            let (action_nodes, action_depth) = action_size(action, &mut errors, index);
            nodes = nodes.saturating_add(action_nodes);
            depth = depth.max(action_depth);
        }
        total_expression_nodes = total_expression_nodes.saturating_add(nodes);
        if depth > MAX_EXPR_DEPTH {
            errors.push(format!(
                "transition {index} exceeds the runtime expression depth limit ({MAX_EXPR_DEPTH})"
            ));
        }
    }
    if total_expression_nodes > MAX_EXPR_NODES {
        errors.push(format!(
            "graph uses {total_expression_nodes} expression nodes; runtime limit is {MAX_EXPR_NODES}"
        ));
    }
    for trigger in &graph.emission_plan.status_triggers {
        check_text_bound(
            &trigger.symbol,
            MAX_SYMBOL_BYTES,
            "status symbol",
            &mut errors,
        );
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn check_text_bound(text: &str, limit: usize, label: &str, errors: &mut Vec<String>) {
    if text.bytes().any(|byte| byte == 0) {
        errors.push(format!("{label} contains a NUL byte"));
    } else if text.len() > limit {
        errors.push(format!("{label} exceeds the {limit}-byte runtime limit"));
    }
}

fn check_predicate_text(predicate: &Predicate, depth: usize, errors: &mut Vec<String>) {
    if depth > super::live_model::MAX_EXPR_DEPTH {
        return;
    }
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
            check_text_bound(
                name,
                super::live_model::MAX_NAME_BYTES,
                "guard variable",
                errors,
            );
        }
        Predicate::EventFlag { event, field } => {
            check_text_bound(
                event,
                super::live_model::MAX_NAME_BYTES,
                "event name",
                errors,
            );
            check_text_bound(
                field,
                super::live_model::MAX_NAME_BYTES,
                "event field name",
                errors,
            );
        }
        Predicate::NativeFlag(source) | Predicate::DamageStatusIs(source) => check_text_bound(
            source.symbol(),
            super::native_inputs::NativeInput::MAX_REFERENCE_BYTES,
            "native input source",
            errors,
        ),
        Predicate::Compare { left, right, .. } => {
            check_expr_text(left, depth + 1, errors);
            check_expr_text(right, depth + 1, errors);
        }
        Predicate::Not(inner) => check_predicate_text(inner, depth + 1, errors),
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            check_predicate_text(left, depth + 1, errors);
            check_predicate_text(right, depth + 1, errors);
        }
    }
}

fn check_expr_text(expression: &Expr, depth: usize, errors: &mut Vec<String>) {
    if depth > super::live_model::MAX_EXPR_DEPTH {
        return;
    }
    match expression {
        Expr::StatusFrame
        | Expr::MotionFrame
        | Expr::StickX { .. }
        | Expr::StickY
        | Expr::RandomInt { .. }
        | Expr::RandomFloat { .. } => {}
        Expr::PendingDamage => {}
        Expr::Const(_) => {}
        Expr::NativeFloat(source) | Expr::NativeInt(source) => check_text_bound(
            source.symbol(),
            super::native_inputs::NativeInput::MAX_REFERENCE_BYTES,
            "native input source",
            errors,
        ),
        Expr::Var(name) => check_text_bound(
            name,
            super::live_model::MAX_NAME_BYTES,
            "expression variable",
            errors,
        ),
        Expr::TimerRemaining(name) => check_text_bound(
            name,
            super::live_model::MAX_NAME_BYTES,
            "timer variable",
            errors,
        ),
        Expr::EventField { event, field } => {
            check_text_bound(
                event,
                super::live_model::MAX_NAME_BYTES,
                "event name",
                errors,
            );
            check_text_bound(
                field,
                super::live_model::MAX_NAME_BYTES,
                "event field name",
                errors,
            );
        }
        Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
            check_expr_text(left, depth + 1, errors);
            check_expr_text(right, depth + 1, errors);
        }
    }
}

fn check_action_text(action: &Action, errors: &mut Vec<String>) {
    match action {
        Action::Status(c) => {
            for e in c.expressions() {
                check_expr_text(e, 1, errors);
            }
        }
        Action::Object { symbol, op } => {
            check_text_bound(
                symbol,
                super::live_model::MAX_SYMBOL_BYTES,
                "article symbol",
                errors,
            );
            for e in op.expressions() {
                check_expr_text(e, 1, errors);
            }
        }
        Action::AddSpeed { x, y } => {
            check_expr_text(x, 1, errors);
            check_expr_text(y, 1, errors);
        }
        Action::SetNative { value, .. } | Action::AddNative { value, .. } => {
            check_expr_text(value, 1, errors)
        }
        Action::SetNativeFlag { value, .. } => check_predicate_text(value, 1, errors),
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
            check_expr_text(amount, 1, errors)
        }
        Action::CombatPolicy { .. }
        | Action::ThrowHeldOpponent { .. }
        | Action::ReleaseHeldOpponent
        | Action::CancelIncomingHit
        | Action::Movement(_)
        | Action::Item(_)
        | Action::MeshVisibility { .. }
        | Action::ChooseHeroMenuSpell { .. } => {}
        Action::EmitEvent { name } => check_text_bound(
            name,
            super::live_model::MAX_NAME_BYTES,
            "event name",
            errors,
        ),
        Action::EmitEventArguments { name, arguments } => {
            check_text_bound(
                name,
                super::live_model::MAX_NAME_BYTES,
                "event name",
                errors,
            );
            if let Err(argument_errors) =
                super::event_payload::validate_argument_shape("graph edit", name, arguments)
            {
                errors.extend(argument_errors.into_iter().map(|error| error.to_string()));
            }
        }
        Action::EmitEventValues { name, arguments } => {
            check_text_bound(
                name,
                super::live_model::MAX_NAME_BYTES,
                "event name",
                errors,
            );
            if name.len() > super::live_model::MAX_NAME_BYTES {
                return;
            }
            if arguments.len() > super::event_payload::MAX_FIELDS_PER_EVENT {
                errors.push(format!(
                    "event has {} arguments; runtime limit is {}",
                    arguments.len(),
                    super::event_payload::MAX_FIELDS_PER_EVENT
                ));
                return;
            }
            for argument in arguments {
                check_text_bound(
                    &argument.field,
                    super::live_model::MAX_NAME_BYTES,
                    "event argument field",
                    errors,
                );
                if argument.field.len() > super::live_model::MAX_NAME_BYTES {
                    return;
                }
            }
            if let Err(argument_errors) =
                super::event_payload::validate_value_argument_shape("graph edit", name, arguments)
            {
                errors.extend(argument_errors.into_iter().map(|error| error.to_string()));
                return;
            }
            for argument in arguments {
                match &argument.value {
                    EventValue::Constant(_) => {}
                    EventValue::Numeric(expression) => check_expr_text(expression, 1, errors),
                    EventValue::Flag(predicate) => check_predicate_text(predicate, 1, errors),
                }
            }
        }
        Action::Set { var, value } | Action::Add { var, value } => {
            check_text_bound(
                var,
                super::live_model::MAX_NAME_BYTES,
                "action variable",
                errors,
            );
            check_expr_text(value, 1, errors);
        }
        Action::SetFlag { var, value } => {
            check_text_bound(
                var,
                super::live_model::MAX_NAME_BYTES,
                "action variable",
                errors,
            );
            check_predicate_text(value, 1, errors);
        }
        Action::Raise { var } | Action::Clear { var } | Action::Reset { var } => {
            check_text_bound(
                var,
                super::live_model::MAX_NAME_BYTES,
                "action variable",
                errors,
            );
        }
        Action::GenerateArticle { symbol }
        | Action::RemoveArticle { symbol }
        | Action::RemoveNewestArticle { symbol }
        | Action::ArticleMovement { symbol, .. } => {
            check_text_bound(
                symbol,
                super::live_model::MAX_SYMBOL_BYTES,
                "article symbol",
                errors,
            );
        }
        Action::RequestArticleStatus { symbol, status } => {
            check_text_bound(
                symbol,
                super::live_model::MAX_SYMBOL_BYTES,
                "article symbol",
                errors,
            );
            check_text_bound(
                status,
                super::live_model::MAX_SYMBOL_BYTES,
                "article status",
                errors,
            );
        }
        Action::RequestFighterStatus { symbol } | Action::RequestWeaponStatus { symbol, .. } => {
            check_text_bound(
                symbol,
                super::live_model::MAX_SYMBOL_BYTES,
                "weapon-status symbol",
                errors,
            );
        }
    }
}

fn expr_size(
    expression: &Expr,
    depth: usize,
    errors: &mut Vec<String>,
    transition: usize,
) -> (usize, usize) {
    if depth > super::live_model::MAX_EXPR_DEPTH {
        errors.push(format!(
            "transition {transition} expression depth exceeds the runtime limit"
        ));
        return (1, depth);
    }
    let here = 1;
    let (nodes, max_depth) = match expression {
        Expr::StatusFrame
        | Expr::MotionFrame
        | Expr::StickX { .. }
        | Expr::StickY
        | Expr::RandomInt { .. }
        | Expr::RandomFloat { .. } => (0, depth),
        Expr::Const(ConstValue::Float(value))
            if !value.is_finite()
                || f64::from(*value).abs() > super::live_model::MAX_ABS_NUMBER =>
        {
            errors.push(format!(
                "transition {transition} contains a non-finite or out-of-range float"
            ));
            (0, depth)
        }
        Expr::Const(ConstValue::Int(value))
            if i64::from(*value).abs() > super::live_model::MAX_ABS_NUMBER as i64 =>
        {
            errors.push(format!(
                "transition {transition} contains an out-of-range integer"
            ));
            (0, depth)
        }
        Expr::PendingDamage
        | Expr::Const(_)
        | Expr::Var(_)
        | Expr::TimerRemaining(_)
        | Expr::EventField { .. }
        | Expr::NativeFloat(_)
        | Expr::NativeInt(_) => (0, depth),
        Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
            let (left_nodes, left_depth) = expr_size(left, depth + 1, errors, transition);
            let (right_nodes, right_depth) = expr_size(right, depth + 1, errors, transition);
            (left_nodes + right_nodes, left_depth.max(right_depth))
        }
    };
    (here + nodes, max_depth)
}

fn predicate_size(
    predicate: &Predicate,
    depth: usize,
    errors: &mut Vec<String>,
    transition: usize,
) -> (usize, usize) {
    if depth > super::live_model::MAX_EXPR_DEPTH {
        errors.push(format!(
            "transition {transition} predicate depth exceeds the runtime limit"
        ));
        return (1, depth);
    }
    let (children, max_depth) = match predicate {
        Predicate::DamageIsProjectile
        | Predicate::InStatus { .. }
        | Predicate::InMotion { .. }
        | Predicate::SituationChanged
        | Predicate::MotionEnded
        | Predicate::CancelEnabled
        | Predicate::TermEnabled { .. }
        | Predicate::Situation { .. }
        | Predicate::Button { .. } => (0, depth),
        Predicate::Flag(_)
        | Predicate::EventFlag { .. }
        | Predicate::Expired(_)
        | Predicate::NativeFlag(_)
        | Predicate::DamageStatusIs(_) => (0, depth),
        Predicate::Compare { left, right, .. } => {
            let (left_nodes, left_depth) = expr_size(left, depth + 1, errors, transition);
            let (right_nodes, right_depth) = expr_size(right, depth + 1, errors, transition);
            (left_nodes + right_nodes, left_depth.max(right_depth))
        }
        Predicate::Not(inner) => {
            let (nodes, depth) = predicate_size(inner, depth + 1, errors, transition);
            (nodes, depth)
        }
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            let (left_nodes, left_depth) = predicate_size(left, depth + 1, errors, transition);
            let (right_nodes, right_depth) = predicate_size(right, depth + 1, errors, transition);
            (left_nodes + right_nodes, left_depth.max(right_depth))
        }
    };
    if depth > super::live_model::MAX_EXPR_DEPTH {
        errors.push(format!(
            "transition {transition} predicate depth exceeds the runtime limit"
        ));
    }
    (1 + children, max_depth)
}

fn action_size(action: &Action, errors: &mut Vec<String>, transition: usize) -> (usize, usize) {
    match action {
        Action::Status(c) => c
            .expressions()
            .into_iter()
            .map(|e| expr_size(e, 1, errors, transition))
            .fold((0, 0), |a, b| (a.0 + b.0, a.1.max(b.1))),
        Action::Object { op, .. } => op.expressions().into_iter().fold((0, 0), |(n, d), e| {
            let (en, ed) = expr_size(e, 1, errors, transition);
            (n + en, d.max(ed))
        }),
        Action::AddSpeed { x, y } => {
            let a = expr_size(x, 1, errors, transition);
            let b = expr_size(y, 1, errors, transition);
            (a.0 + b.0, a.1.max(b.1))
        }
        Action::SetNative { value, .. } | Action::AddNative { value, .. } => {
            expr_size(value, 1, errors, transition)
        }
        Action::SetNativeFlag { value, .. } => predicate_size(value, 1, errors, transition),
        Action::Set { value, .. }
        | Action::Add { value, .. }
        | Action::SetMotionRate { rate: value }
        | Action::AddHeldOpponentDamage { amount: value }
        | Action::AddDamage { amount: value }
        | Action::CombatPolicy {
            op:
                CombatOp::DamageTaken(value)
                | CombatOp::KnockbackTaken(value)
                | CombatOp::DamageDealt(value)
                | CombatOp::KnockbackDealt(value)
                | CombatOp::Stat(_, value)
                | CombatOp::Armor(value),
            ..
        }
        | Action::Resource(crate::gimmicks::resource_commands::Command { amount: value, .. }) => {
            let (nodes, depth) = expr_size(value, 1, errors, transition);
            (nodes, depth)
        }
        Action::SetFlag { value, .. } => predicate_size(value, 1, errors, transition),
        Action::Raise { .. }
        | Action::Clear { .. }
        | Action::Reset { .. }
        | Action::GenerateArticle { .. }
        | Action::RemoveArticle { .. }
        | Action::RemoveNewestArticle { .. }
        | Action::ArticleMovement { .. }
        | Action::RequestArticleStatus { .. }
        | Action::EmitEvent { .. }
        | Action::RequestFighterStatus { .. }
        | Action::RequestWeaponStatus { .. }
        | Action::CombatPolicy { .. }
        | Action::ThrowHeldOpponent { .. }
        | Action::ReleaseHeldOpponent
        | Action::CancelIncomingHit
        | Action::Movement(_)
        | Action::Item(_)
        | Action::MeshVisibility { .. }
        | Action::ChooseHeroMenuSpell { .. } => (0, 1),
        Action::EmitEventArguments { arguments, .. } => (
            arguments
                .len()
                .min(super::event_payload::MAX_FIELDS_PER_EVENT),
            1,
        ),
        Action::EmitEventValues { arguments, .. } => {
            let mut nodes = 0usize;
            let mut max_depth = 1usize;
            for argument in arguments
                .iter()
                .take(super::event_payload::MAX_FIELDS_PER_EVENT)
            {
                let (argument_nodes, argument_depth) = match &argument.value {
                    EventValue::Constant(_) => (0, 1),
                    EventValue::Numeric(expression) => expr_size(expression, 1, errors, transition),
                    EventValue::Flag(predicate) => predicate_size(predicate, 1, errors, transition),
                };
                nodes = nodes.saturating_add(argument_nodes);
                max_depth = max_depth.max(argument_depth);
            }
            (nodes, max_depth)
        }
    }
}

fn remap_move(index: usize, from: usize, to: usize) -> usize {
    if index == from {
        to
    } else if from < to && (from + 1..=to).contains(&index) {
        index - 1
    } else if to < from && (to..from).contains(&index) {
        index + 1
    } else {
        index
    }
}

fn rewrite_expr(expression: &mut Expr, old: &str, new: &str) {
    match expression {
        Expr::StatusFrame
        | Expr::MotionFrame
        | Expr::StickX { .. }
        | Expr::StickY
        | Expr::RandomInt { .. }
        | Expr::RandomFloat { .. } => {}
        Expr::PendingDamage => {}
        Expr::Const(_) | Expr::NativeFloat(_) | Expr::NativeInt(_) | Expr::EventField { .. } => {}
        Expr::Var(name) => {
            if name == old {
                *name = new.to_owned();
            }
        }
        Expr::TimerRemaining(name) => {
            if name == old {
                *name = new.to_owned();
            }
        }
        Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
            rewrite_expr(left, old, new);
            rewrite_expr(right, old, new);
        }
    }
}

fn rewrite_predicate(predicate: &mut Predicate, old: &str, new: &str) {
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
        Predicate::NativeFlag(_) | Predicate::DamageStatusIs(_) | Predicate::EventFlag { .. } => {}
        Predicate::Flag(name) | Predicate::Expired(name) => {
            if name == old {
                *name = new.to_owned();
            }
        }
        Predicate::Compare { left, right, .. } => {
            rewrite_expr(left, old, new);
            rewrite_expr(right, old, new);
        }
        Predicate::Not(inner) => rewrite_predicate(inner, old, new),
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            rewrite_predicate(left, old, new);
            rewrite_predicate(right, old, new);
        }
    }
}

fn rewrite_action(action: &mut Action, old: &str, new: &str) {
    match action {
        Action::Status(c) => {
            for e in c.expressions_mut() {
                rewrite_expr(e, old, new);
            }
        }
        Action::Object { op, .. } => {
            for e in op.expressions_mut() {
                rewrite_expr(e, old, new);
            }
        }
        Action::AddSpeed { x, y } => {
            rewrite_expr(x, old, new);
            rewrite_expr(y, old, new);
        }
        Action::SetNative { value, .. } | Action::AddNative { value, .. } => {
            rewrite_expr(value, old, new)
        }
        Action::SetNativeFlag { value, .. } => rewrite_predicate(value, old, new),
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
            rewrite_expr(amount, old, new)
        }
        Action::Set { var, value } | Action::Add { var, value } => {
            if var == old {
                *var = new.to_owned();
            }
            rewrite_expr(value, old, new);
        }
        Action::SetFlag { var, value } => {
            if var == old {
                *var = new.to_owned();
            }
            rewrite_predicate(value, old, new);
        }
        Action::Raise { var } | Action::Clear { var } | Action::Reset { var } => {
            if var == old {
                *var = new.to_owned();
            }
        }
        Action::EmitEventValues { arguments, .. } => {
            for argument in arguments {
                match &mut argument.value {
                    EventValue::Constant(_) => {}
                    EventValue::Numeric(expression) => rewrite_expr(expression, old, new),
                    EventValue::Flag(predicate) => rewrite_predicate(predicate, old, new),
                }
            }
        }
        Action::GenerateArticle { .. }
        | Action::RemoveArticle { .. }
        | Action::RemoveNewestArticle { .. }
        | Action::ArticleMovement { .. }
        | Action::RequestArticleStatus { .. }
        | Action::EmitEvent { .. }
        | Action::EmitEventArguments { .. }
        | Action::RequestFighterStatus { .. }
        | Action::RequestWeaponStatus { .. }
        | Action::CombatPolicy { .. }
        | Action::ThrowHeldOpponent { .. }
        | Action::ReleaseHeldOpponent
        | Action::CancelIncomingHit
        | Action::Movement(_)
        | Action::Item(_)
        | Action::MeshVisibility { .. }
        | Action::ChooseHeroMenuSpell { .. } => {}
    }
}

fn expr_references(expression: &Expr, name: &str) -> bool {
    match expression {
        Expr::StatusFrame
        | Expr::MotionFrame
        | Expr::StickX { .. }
        | Expr::StickY
        | Expr::RandomInt { .. }
        | Expr::RandomFloat { .. } => false,
        Expr::PendingDamage => false,
        Expr::Const(_) | Expr::NativeFloat(_) | Expr::NativeInt(_) | Expr::EventField { .. } => {
            false
        }
        Expr::Var(variable) => variable == name,
        Expr::TimerRemaining(variable) => variable == name,
        Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
            expr_references(left, name) || expr_references(right, name)
        }
    }
}

fn predicate_references(predicate: &Predicate, name: &str) -> bool {
    match predicate {
        Predicate::DamageIsProjectile
        | Predicate::InStatus { .. }
        | Predicate::InMotion { .. }
        | Predicate::SituationChanged
        | Predicate::MotionEnded
        | Predicate::CancelEnabled
        | Predicate::TermEnabled { .. }
        | Predicate::Situation { .. }
        | Predicate::Button { .. } => false,
        Predicate::NativeFlag(_) | Predicate::DamageStatusIs(_) | Predicate::EventFlag { .. } => {
            false
        }
        Predicate::Flag(variable) | Predicate::Expired(variable) => variable == name,
        Predicate::Compare { left, right, .. } => {
            expr_references(left, name) || expr_references(right, name)
        }
        Predicate::Not(inner) => predicate_references(inner, name),
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            predicate_references(left, name) || predicate_references(right, name)
        }
    }
}

fn action_references(action: &Action, name: &str) -> bool {
    match action {
        Action::Status(c) => c.expressions().iter().any(|e| expr_references(e, name)),
        Action::Object { op, .. } => op
            .expressions()
            .into_iter()
            .any(|e| expr_references(e, name)),
        Action::AddSpeed { x, y } => expr_references(x, name) || expr_references(y, name),
        Action::SetNative { value, .. } | Action::AddNative { value, .. } => {
            expr_references(value, name)
        }
        Action::SetNativeFlag { value, .. } => predicate_references(value, name),
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
            expr_references(amount, name)
        }
        Action::Set { var, value } | Action::Add { var, value } => {
            var == name || expr_references(value, name)
        }
        Action::SetFlag { var, value } => var == name || predicate_references(value, name),
        Action::Raise { var } | Action::Clear { var } | Action::Reset { var } => var == name,
        Action::EmitEventValues { arguments, .. } => {
            arguments.iter().any(|argument| match &argument.value {
                EventValue::Constant(_) => false,
                EventValue::Numeric(expression) => expr_references(expression, name),
                EventValue::Flag(predicate) => predicate_references(predicate, name),
            })
        }
        Action::GenerateArticle { .. }
        | Action::RemoveArticle { .. }
        | Action::RemoveNewestArticle { .. }
        | Action::ArticleMovement { .. }
        | Action::RequestArticleStatus { .. }
        | Action::EmitEvent { .. }
        | Action::EmitEventArguments { .. }
        | Action::RequestFighterStatus { .. }
        | Action::RequestWeaponStatus { .. }
        | Action::CombatPolicy { .. }
        | Action::ThrowHeldOpponent { .. }
        | Action::ReleaseHeldOpponent
        | Action::CancelIncomingHit
        | Action::Movement(_)
        | Action::Item(_)
        | Action::MeshVisibility { .. }
        | Action::ChooseHeroMenuSpell { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::behavior::{CmpOp, ConstValue, StateDef, VarType};
    use crate::gimmicks::codegen::EmissionPlan;

    fn base_graph() -> BehaviorGraph {
        BehaviorGraph {
            costumes: Vec::new(),
            custom_statuses: Default::default(),
            initial: "idle".into(),
            states: vec![
                StateDef {
                    status: None,
                    name: "idle".into(),
                },
                StateDef {
                    status: None,
                    name: "active".into(),
                },
            ],
            vars: vec![VarDef {
                keep_across_form_switch: false,
                name: "gauge".into(),
                kind: VarType::Float,
                initial: ConstValue::Float(0.0),
            }],
            transitions: vec![Transition {
                from: "idle".into(),
                to: "active".into(),
                guard: Predicate::Compare {
                    op: CmpOp::Gte,
                    left: Expr::Var("gauge".into()),
                    right: Expr::Const(ConstValue::Float(1.0)),
                },
                on_frame: true,
                actions: vec![Action::Add {
                    var: "gauge".into(),
                    value: Expr::Const(ConstValue::Float(0.5)),
                }],
            }],
            emission_plan: EmissionPlan::default(),
        }
    }

    #[test]
    fn custom_owner_round_trip_keeps_deleted_keys_reserved() {
        let mut graph = BehaviorGraph {
            initial: "idle".into(),
            states: vec![
                StateDef {
                    name: "idle".into(),
                    status: None,
                },
                StateDef {
                    name: "Custom pose".into(),
                    status: None,
                },
            ],
            ..Default::default()
        };
        let target =
            super::super::status_model::allocate(&mut graph.custom_statuses, "Custom pose")
                .unwrap();
        let owned =
            set_state_status_ref(&graph, "mario", "Custom pose", Some(target.clone())).unwrap();
        let restored: BehaviorGraph =
            serde_json::from_str(&serde_json::to_string(&owned).unwrap()).unwrap();
        assert_eq!(restored, owned);
        let released = set_state_status_ref(&restored, "mario", "Custom pose", None).unwrap();
        assert_eq!(released.custom_statuses, owned.custom_statuses);
        assert_eq!(target.resolve("mario").unwrap(), 0x1e4);
        assert!(crate::gimmicks::behavior::validate_for_fighter("mario", &restored).is_empty());
    }

    #[test]
    fn replace_action_rejects_wrong_fighter_symbols_atomically() {
        let original = base_graph();
        let cloud_symbol = crate::gimmicks::registry::entries_for(
            "cloud",
            crate::gimmicks::registry::IdentifierKind::Article,
        )
        .next()
        .unwrap()
        .symbol;
        let error = replace_action(
            &original,
            "mario",
            0,
            0,
            Action::GenerateArticle {
                symbol: cloud_symbol.into(),
            },
        )
        .unwrap_err();
        assert!(error.contains("transition 0"), "{error}");
        assert!(error.contains("action 0"), "{error}");
        assert!(error.contains("WrongFighter"), "{error}");
        assert_eq!(original, base_graph());
    }

    #[test]
    fn event_transitions_share_a_receiver_and_renames_reject_duplicates() {
        let step = |graph: &BehaviorGraph, name| {
            let transition = graph.transitions[0].clone();
            add_event_transition(graph, "mario", transition, name).unwrap()
        };
        let mut start = base_graph();
        start
            .emission_plan
            .event_bindings
            .push(crate::gimmicks::codegen::EventBinding {
                name: "jab".into(),
                transitions: vec![0],
                fields: Vec::new(),
            });
        let graph = step(&step(&start, "jab"), "spend");
        let bindings = &graph.emission_plan.event_bindings;
        assert_eq!(bindings.len(), 2);
        assert_eq!(
            (bindings[0].name.as_str(), bindings[0].transitions.clone()),
            ("jab", vec![0, 1])
        );
        assert_eq!(
            (bindings[1].name.as_str(), bindings[1].transitions.clone()),
            ("spend", vec![2])
        );

        let renamed = rename_event(&graph, "mario", 1, "reset").unwrap();
        assert_eq!(renamed.emission_plan.event_bindings[1].name, "reset");
        assert!(rename_event(&graph, "mario", 1, "jab")
            .unwrap_err()
            .contains("already receives"));
        assert!(rename_event(&graph, "mario", 1, "").is_err());
        assert!(rename_event(&graph, "mario", 9, "other").is_err());
    }

    #[test]
    fn owning_a_status_adds_installable_rules_and_releasing_removes_them() {
        use super::super::status_model::Phase;
        let mut graph = base_graph();
        graph.transitions.clear();
        let graph = add_state(&graph, "mario", "cape").unwrap();
        let before = graph.transitions.len();
        assert!(set_state_status(
            &graph,
            "mario",
            "idle",
            Some("FIGHTER_STATUS_KIND_SPECIAL_S")
        )
        .is_err());
        assert!(set_state_status(
            &graph,
            "mario",
            "cape",
            Some("FIGHTER_CLOUD_STATUS_KIND_FINAL2")
        )
        .is_err());
        let owned = set_state_status(
            &graph,
            "mario",
            "cape",
            Some("FIGHTER_STATUS_KIND_SPECIAL_S"),
        )
        .unwrap();
        assert_eq!(owned.transitions.len(), before + 4);
        assert!(
            matches!(&owned.transitions[before + 1].actions[0], Action::Status(super::super::status_commands::Command::PlayMotion {motion,..}) if motion == "wait")
        );
        assert!(super::super::live_protocol::program_from_graph("mario", &owned).is_ok());
        let binding = owned
            .states
            .iter()
            .find_map(|state| state.status.clone())
            .unwrap();
        assert_eq!(
            binding.roots[&Phase::MainLoop],
            vec![before + 2, before + 3]
        );
        let retargeted = set_state_status(
            &owned,
            "mario",
            "cape",
            Some("FIGHTER_STATUS_KIND_SPECIAL_N"),
        )
        .unwrap();
        assert_eq!(retargeted.transitions.len(), owned.transitions.len());
        let moved = reorder_transition(&owned, "mario", before + 2, 0).unwrap();
        let roots = &moved
            .states
            .iter()
            .find_map(|state| state.status.clone())
            .unwrap()
            .roots;
        assert_eq!((roots[&Phase::MainLoop][0], roots[&Phase::Pre][0]), (0, 1));
        let released = set_state_status(&moved, "mario", "cape", None).unwrap();
        assert_eq!(released.transitions, graph.transitions);
        assert!(released.states.iter().all(|state| state.status.is_none()));
    }

    #[test]
    fn state_rename_rewrites_entry_and_transition_endpoints_atomically() {
        let original = base_graph();
        let renamed = rename_state(&original, "mario", "idle", "ready").unwrap();
        assert_eq!(renamed.initial, "ready");
        assert_eq!(renamed.transitions[0].from, "ready");
        assert_eq!(original.initial, "idle");
        assert_eq!(original.transitions[0].from, "idle");
    }

    #[test]
    fn variable_rename_rewrites_nested_guards_and_all_action_forms() {
        let mut original = base_graph();
        original.transitions[0].guard = Predicate::And(
            Box::new(Predicate::Flag("armed".into())),
            Box::new(Predicate::Compare {
                op: CmpOp::Gte,
                left: Expr::Add(
                    Box::new(Expr::Var("gauge".into())),
                    Box::new(Expr::Const(ConstValue::Float(2.0))),
                ),
                right: Expr::Const(ConstValue::Float(1.0)),
            }),
        );
        original.vars.push(VarDef {
            keep_across_form_switch: false,
            name: "armed".into(),
            kind: VarType::Flag,
            initial: ConstValue::Flag(false),
        });
        original.transitions[0].actions.extend([
            Action::Raise {
                var: "armed".into(),
            },
            Action::Set {
                var: "gauge".into(),
                value: Expr::Mul(
                    Box::new(Expr::Var("gauge".into())),
                    Box::new(Expr::Const(ConstValue::Float(2.0))),
                ),
            },
            Action::EmitEventValues {
                name: "charged".into(),
                arguments: vec![
                    super::super::event_payload::ValueArgument {
                        field: "amount".into(),
                        value: EventValue::Numeric(Expr::Add(
                            Box::new(Expr::Var("gauge".into())),
                            Box::new(Expr::Const(ConstValue::Float(1.0))),
                        )),
                    },
                    super::super::event_payload::ValueArgument {
                        field: "enabled".into(),
                        value: EventValue::Flag(Predicate::And(
                            Box::new(Predicate::Flag("armed".into())),
                            Box::new(Predicate::Compare {
                                op: CmpOp::Gt,
                                left: Expr::Var("gauge".into()),
                                right: Expr::Const(ConstValue::Float(0.0)),
                            }),
                        )),
                    },
                ],
            },
        ]);
        let renamed = rename_variable(&original, "mario", "gauge", "charge").unwrap();
        assert_eq!(renamed.vars[0].name, "charge");
        assert!(matches!(
            &renamed.transitions[0].guard,
            Predicate::And(_, inner) if matches!(
                inner.as_ref(),
                Predicate::Compare { left: Expr::Add(left, _), .. }
                    if matches!(left.as_ref(), Expr::Var(name) if name == "charge")
            )
        ));
        assert!(matches!(
            &renamed.transitions[0].actions[0],
            Action::Add { var, .. } if var == "charge"
        ));
        assert!(matches!(
            &renamed.transitions[0].actions[2],
            Action::Set { value: Expr::Mul(left, _), .. }
                if matches!(left.as_ref(), Expr::Var(name) if name == "charge")
        ));
        let Action::EmitEventValues { arguments, .. } = &renamed.transitions[0].actions[3] else {
            panic!("dynamic event action was not preserved")
        };
        assert!(matches!(
            &arguments[0].value,
            EventValue::Numeric(Expr::Add(left, _))
                if matches!(left.as_ref(), Expr::Var(name) if name == "charge")
        ));
        assert!(matches!(
            &arguments[1].value,
            EventValue::Flag(Predicate::And(_, inner))
                if matches!(inner.as_ref(), Predicate::Compare { left: Expr::Var(name), .. } if name == "charge")
        ));
    }

    #[test]
    fn variable_removal_rejects_nested_guard_or_action_references_atomically() {
        let original = base_graph();
        assert!(remove_variable(&original, "mario", "gauge").is_err());
        assert_eq!(original.vars.len(), 1);
    }

    #[test]
    fn transition_add_and_replace_validate_emitted_plan() {
        let mut empty = BehaviorGraph::default();
        empty = add_state(&empty, "mario", "idle").unwrap();
        empty = add_state(&empty, "mario", "active").unwrap();
        empty = add_variable(
            &empty,
            "mario",
            VarDef {
                keep_across_form_switch: false,
                name: "gauge".into(),
                kind: VarType::Float,
                initial: ConstValue::Float(0.0),
            },
        )
        .unwrap();
        let transition = base_graph().transitions.remove(0);
        let added =
            add_transition(&empty, "mario", transition.clone(), HookName::OnChargeTick).unwrap();
        assert_eq!(added.emission_plan.bindings[0].transitions, vec![0]);
        let replaced = replace_transition(&added, "mario", 0, transition).unwrap();
        assert_eq!(
            replaced.emission_plan.bindings,
            added.emission_plan.bindings
        );
    }

    #[test]
    fn exact_status_edit_keeps_enter_exit_and_each_symbol_scoped() {
        let transition = || Transition {
            from: "idle".into(),
            to: "idle".into(),
            guard: Predicate::Compare {
                op: CmpOp::Eq,
                left: Expr::Const(ConstValue::Int(0)),
                right: Expr::Const(ConstValue::Int(0)),
            },
            on_frame: true,
            actions: vec![],
        };
        let symbol_a = "FIGHTER_CLOUD_STATUS_KIND_SPECIAL_LW_CHARGE";
        let symbol_b = "FIGHTER_CLOUD_STATUS_KIND_FINAL2";
        let mut original = base_graph();
        original.emission_plan.bindings.push(HookBinding {
            hook: HookName::OnChargeTick,
            article: None,
            transitions: vec![0],
        });
        let before = original.clone();
        let entered = add_status_transition(
            &original,
            "cloud",
            transition(),
            HookName::OnStatusEnter,
            symbol_a,
        )
        .unwrap();
        let exited_same = add_status_transition(
            &entered,
            "cloud",
            transition(),
            HookName::OnStatusExit,
            symbol_a,
        )
        .unwrap();
        let exited_other = add_status_transition(
            &exited_same,
            "cloud",
            transition(),
            HookName::OnStatusExit,
            symbol_b,
        )
        .unwrap();

        let enter = exited_other
            .emission_plan
            .status_triggers
            .iter()
            .find(|trigger| trigger.hook == HookName::OnStatusEnter)
            .unwrap();
        let exit_a = exited_other
            .emission_plan
            .status_triggers
            .iter()
            .find(|trigger| trigger.hook == HookName::OnStatusExit && trigger.symbol == symbol_a)
            .unwrap();
        let exit_b = exited_other
            .emission_plan
            .status_triggers
            .iter()
            .find(|trigger| trigger.hook == HookName::OnStatusExit && trigger.symbol == symbol_b)
            .unwrap();
        assert_eq!(enter.transitions, vec![1]);
        assert_eq!(exit_a.transitions, vec![2]);
        assert_eq!(exit_b.transitions, vec![3]);
        assert_eq!(enter.evidence, "");
        assert!(!enter.runtime_validated);
        assert!(validate_graph(&exited_other, "cloud").is_ok());
        assert_eq!(original, before);

        for (hook, symbol, event_boundary) in [
            (HookName::OnStatusExit, "NO_SUCH_STATUS", true),
            (HookName::OnSpend, symbol_a, true),
            (HookName::OnStatusExit, symbol_a, false),
        ] {
            assert!(add_status_transition(
                &original,
                "cloud",
                Transition {
                    on_frame: event_boundary,
                    ..transition()
                },
                hook,
                symbol,
            )
            .is_err());
            assert_eq!(original, before);
        }
    }

    #[test]
    fn delete_and_reorder_rewrite_indices_preserving_binding_order() {
        let mut graph = base_graph();
        graph.transitions.push(graph.transitions[0].clone());
        graph.transitions.push(graph.transitions[0].clone());
        graph.emission_plan.bindings = vec![
            HookBinding {
                hook: HookName::OnChargeTick,
                article: None,
                transitions: vec![0, 2],
            },
            HookBinding {
                hook: HookName::OnStockStart,
                article: None,
                transitions: vec![1],
            },
        ];
        let moved = reorder_transition(&graph, "mario", 0, 2).unwrap();
        assert_eq!(moved.emission_plan.bindings[0].transitions, vec![2, 1]);
        assert_eq!(moved.emission_plan.bindings[1].transitions, vec![0]);
        let deleted = delete_transition(&moved, "mario", 1).unwrap();
        assert_eq!(deleted.emission_plan.bindings[0].transitions, vec![1]);
        assert_eq!(deleted.emission_plan.bindings[1].transitions, vec![0]);
    }
}
