//! Bounded, lossless references from character behavior records to field items.
//!
//! This module does not own or normalize behavior. A projection names records in
//! [`BehaviorGraph`](super::behavior::BehaviorGraph), preserves their declaration
//! and emission order, and reports dangling references instead of dropping them.

use std::collections::HashSet;

use super::behavior::{self, Action, BehaviorGraph, Expr, Predicate, Transition, VarType};
use super::codegen::{self, EmissionPlan};
use super::event_payload::EventValue;
use super::field_values::{ExprPath, PredicatePath, ValueRoot};
use super::graph_edit;
use super::live_model;
use super::native_inputs::CombatOp;

const MAX_FIELD_ITEMS: usize = 8_192;
const MAX_EVIDENCE_BYTES: usize = 4_096;
const MAX_KEY_BYTES: usize = 512;

/// Stable identity for a semantic behavior record or an explicit missing reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FieldRef {
    Graph,
    State(String),
    Transition(usize),
    Action {
        transition: usize,
        index: usize,
    },
    Variable(String),
    /// Receiver-scoped event field, distinct from a behavior variable.
    EventField {
        event: String,
        field: String,
    },
    Expression {
        root: ValueRoot,
        path: ExprPath,
    },
    Predicate {
        transition: usize,
        path: PredicatePath,
    },
    SourceBinding(SourceBindingRef),
    MissingBinding {
        transition: usize,
    },
    MissingTransition {
        source: SourceBindingRef,
        transition: usize,
    },
    MissingState(String),
    MissingVariable(String),
    MissingEventBinding(String),
    MissingSourceHook {
        source: SourceBindingRef,
    },
}

/// One typed emission-plan record, addressed by its original vector index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SourceBindingRef {
    Hook(usize),
    Event(usize),
    Status(usize),
    Resource(usize),
}

/// A field item references one canonical record; payload remains in the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldNode {
    pub reference: FieldRef,
}

/// The meaning of one typed connection in the projected graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldEdgeKind {
    EntryState,
    TransitionFrom,
    TransitionTo,
    GuardVariableRead,
    EventFieldRead,
    ActionVariableTarget,
    ActionValueRead,
    /// Explicit scalar timer read, distinct from a timer/condition reference.
    TimerRemainingRead,
    ActionSequence,
    SourceHook,
    EventDispatch,
    HookTransition,
    EventTransition,
    StatusTransition,
    MissingBinding,
}

/// A connection between semantic references. Entry edges have no `from` node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldEdge {
    pub from: Option<FieldRef>,
    pub to: Option<FieldRef>,
    pub kind: FieldEdgeKind,
}

/// A projection warning or invalid-reference report, tied to the affected item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDiagnostic {
    pub reference: FieldRef,
    pub message: String,
}

/// Read-only field view of a canonical character behavior graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldProjection {
    pub nodes: Vec<FieldNode>,
    pub edges: Vec<FieldEdge>,
    pub diagnostics: Vec<FieldDiagnostic>,
}

/// Which endpoint of a transition a state connection edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionEndpoint {
    From,
    To,
}

/// Build a bounded projection without cloning the source graph.
///
/// All existing emission-plan entries are represented, including empty or
/// dangling bindings. A transition not owned by a hook/event receiver receives
/// an explicit `MissingBinding` node and diagnostic.
pub fn project(graph: &BehaviorGraph) -> Result<FieldProjection, String> {
    preflight(graph)?;

    let mut projection = FieldProjection {
        nodes: Vec::new(),
        edges: Vec::new(),
        diagnostics: Vec::new(),
    };
    let mut known_nodes = HashSet::new();
    add_node(&mut projection, &mut known_nodes, FieldRef::Graph);

    for state in &graph.states {
        add_node(
            &mut projection,
            &mut known_nodes,
            FieldRef::State(state.name.clone()),
        );
    }
    for (index, transition) in graph.transitions.iter().enumerate() {
        add_node(
            &mut projection,
            &mut known_nodes,
            FieldRef::Transition(index),
        );
        for action_index in 0..transition.actions.len() {
            add_node(
                &mut projection,
                &mut known_nodes,
                FieldRef::Action {
                    transition: index,
                    index: action_index,
                },
            );
        }
    }
    for variable in &graph.vars {
        add_node(
            &mut projection,
            &mut known_nodes,
            FieldRef::Variable(variable.name.clone()),
        );
    }
    add_source_nodes(&mut projection, &mut known_nodes, &graph.emission_plan);

    if !graph.initial.is_empty() {
        let state = state_reference(graph, &graph.initial, &mut projection, &mut known_nodes);
        projection.edges.push(FieldEdge {
            from: None,
            to: Some(state),
            kind: FieldEdgeKind::EntryState,
        });
    }

    let mut owners = vec![false; graph.transitions.len()];
    for (transition_index, transition) in graph.transitions.iter().enumerate() {
        let transition_ref = FieldRef::Transition(transition_index);
        let from = state_reference(graph, &transition.from, &mut projection, &mut known_nodes);
        let to = state_reference(graph, &transition.to, &mut projection, &mut known_nodes);
        let phase = super::status_model::phase(
            graph.states.iter().filter_map(|s| s.status.as_ref()),
            transition_index,
        );
        projection.edges.push(FieldEdge {
            from: Some(if phase.is_some() {
                from.clone()
            } else {
                transition_ref.clone()
            }),
            to: Some(if phase.is_some() {
                transition_ref.clone()
            } else {
                from
            }),
            kind: FieldEdgeKind::TransitionFrom,
        });
        if phase.is_none() || transition.from != transition.to {
            projection.edges.push(FieldEdge {
                from: Some(transition_ref.clone()),
                to: Some(to),
                kind: FieldEdgeKind::TransitionTo,
            });
        }
        for (variable, kind) in predicate_variable_reads(&transition.guard) {
            let target = variable_reference(graph, variable, &mut projection, &mut known_nodes);
            projection.edges.push(FieldEdge {
                from: Some(transition_ref.clone()),
                to: Some(target),
                kind,
            });
        }
        for (event, field) in predicate_event_fields(&transition.guard) {
            let target = FieldRef::EventField {
                event: event.to_owned(),
                field: field.to_owned(),
            };
            add_node(&mut projection, &mut known_nodes, target.clone());
            projection.edges.push(FieldEdge {
                from: Some(transition_ref.clone()),
                to: Some(target),
                kind: FieldEdgeKind::EventFieldRead,
            });
        }
        for (action_index, action) in transition.actions.iter().enumerate() {
            let action_ref = FieldRef::Action {
                transition: transition_index,
                index: action_index,
            };
            if let Some(target_name) = action_target(action) {
                let target =
                    variable_reference(graph, target_name, &mut projection, &mut known_nodes);
                projection.edges.push(FieldEdge {
                    from: Some(action_ref.clone()),
                    to: Some(target),
                    kind: FieldEdgeKind::ActionVariableTarget,
                });
            }
            if let Some(value) = action_value(action) {
                for (variable, kind) in expression_variable_reads(value) {
                    let target =
                        variable_reference(graph, variable, &mut projection, &mut known_nodes);
                    projection.edges.push(FieldEdge {
                        from: Some(action_ref.clone()),
                        to: Some(target),
                        kind,
                    });
                }
                for (event, field) in expression_event_fields(value) {
                    let target = FieldRef::EventField {
                        event: event.to_owned(),
                        field: field.to_owned(),
                    };
                    add_node(&mut projection, &mut known_nodes, target.clone());
                    projection.edges.push(FieldEdge {
                        from: Some(action_ref.clone()),
                        to: Some(target),
                        kind: FieldEdgeKind::EventFieldRead,
                    });
                }
            }
            if let Action::SetFlag { value, .. } = action {
                for (variable, kind) in predicate_variable_reads(value) {
                    let target =
                        variable_reference(graph, variable, &mut projection, &mut known_nodes);
                    projection.edges.push(FieldEdge {
                        from: Some(action_ref.clone()),
                        to: Some(target),
                        kind,
                    });
                }
                for (event, field) in predicate_event_fields(value) {
                    let target = FieldRef::EventField {
                        event: event.into(),
                        field: field.into(),
                    };
                    add_node(&mut projection, &mut known_nodes, target.clone());
                    projection.edges.push(FieldEdge {
                        from: Some(action_ref.clone()),
                        to: Some(target),
                        kind: FieldEdgeKind::EventFieldRead,
                    });
                }
            }
            // Computed payload operands are dependencies of the containing
            // action. They are intentionally not represented as editable
            // Expression/Predicate nodes until those values have field roots.
            if let Action::EmitEventValues { arguments, .. } = action {
                for argument in arguments {
                    match &argument.value {
                        EventValue::Constant(_) => {}
                        EventValue::Numeric(expression) => {
                            for (variable, kind) in expression_variable_reads(expression) {
                                let target = variable_reference(
                                    graph,
                                    variable,
                                    &mut projection,
                                    &mut known_nodes,
                                );
                                projection.edges.push(FieldEdge {
                                    from: Some(action_ref.clone()),
                                    to: Some(target),
                                    kind,
                                });
                            }
                            for (event, field) in expression_event_fields(expression) {
                                let target = FieldRef::EventField {
                                    event: event.to_owned(),
                                    field: field.to_owned(),
                                };
                                add_node(&mut projection, &mut known_nodes, target.clone());
                                projection.edges.push(FieldEdge {
                                    from: Some(action_ref.clone()),
                                    to: Some(target),
                                    kind: FieldEdgeKind::EventFieldRead,
                                });
                            }
                        }
                        EventValue::Flag(predicate) => {
                            for (variable, kind) in predicate_variable_reads(predicate) {
                                let target = variable_reference(
                                    graph,
                                    variable,
                                    &mut projection,
                                    &mut known_nodes,
                                );
                                projection.edges.push(FieldEdge {
                                    from: Some(action_ref.clone()),
                                    to: Some(target),
                                    kind: if kind == FieldEdgeKind::TimerRemainingRead {
                                        kind
                                    } else {
                                        FieldEdgeKind::ActionValueRead
                                    },
                                });
                            }
                            for (event, field) in predicate_event_fields(predicate) {
                                let target = FieldRef::EventField {
                                    event: event.to_owned(),
                                    field: field.to_owned(),
                                };
                                add_node(&mut projection, &mut known_nodes, target.clone());
                                projection.edges.push(FieldEdge {
                                    from: Some(action_ref.clone()),
                                    to: Some(target),
                                    kind: FieldEdgeKind::EventFieldRead,
                                });
                            }
                        }
                    }
                }
            }
            if action_index == 0 {
                projection.edges.push(FieldEdge {
                    from: Some(transition_ref.clone()),
                    to: Some(action_ref.clone()),
                    kind: FieldEdgeKind::ActionSequence,
                });
            } else {
                projection.edges.push(FieldEdge {
                    from: Some(FieldRef::Action {
                        transition: transition_index,
                        index: action_index - 1,
                    }),
                    to: Some(action_ref.clone()),
                    kind: FieldEdgeKind::ActionSequence,
                });
            }
            if let Some(name) = emitted_event_name(action) {
                let matching = graph
                    .emission_plan
                    .event_bindings
                    .iter()
                    .enumerate()
                    .filter(|(_, binding)| binding.name == *name)
                    .map(|(index, _)| FieldRef::SourceBinding(SourceBindingRef::Event(index)))
                    .collect::<Vec<_>>();
                if matching.is_empty() {
                    let missing = FieldRef::MissingEventBinding(name.to_owned());
                    add_node(&mut projection, &mut known_nodes, missing.clone());
                    projection.edges.push(FieldEdge {
                        from: Some(action_ref.clone()),
                        to: Some(missing.clone()),
                        kind: FieldEdgeKind::EventDispatch,
                    });
                    projection.diagnostics.push(FieldDiagnostic {
                        reference: missing,
                        message: format!("event {:?} has no receiver binding", name),
                    });
                } else {
                    for target in matching {
                        projection.edges.push(FieldEdge {
                            from: Some(action_ref.clone()),
                            to: Some(target),
                            kind: FieldEdgeKind::EventDispatch,
                        });
                    }
                }
            }
        }
    }

    // Phase roots have a state dispatcher rather than an ordinary hook receiver.
    for state in &graph.states {
        if let Some(binding) = &state.status {
            for &index in binding.roots.values().flatten() {
                if let Some(owned) = owners.get_mut(index) {
                    *owned = true;
                }
            }
        }
    }
    project_binding_edges(graph, &mut projection, &mut known_nodes, &mut owners);
    project_source_hooks(graph, &mut projection, &mut known_nodes);
    for (index, owned) in owners.into_iter().enumerate() {
        if !owned {
            let reference = FieldRef::MissingBinding { transition: index };
            add_node(&mut projection, &mut known_nodes, reference.clone());
            projection.edges.push(FieldEdge {
                from: Some(reference.clone()),
                to: Some(FieldRef::Transition(index)),
                kind: FieldEdgeKind::MissingBinding,
            });
            projection.diagnostics.push(FieldDiagnostic {
                reference,
                message: "transition is not bound to a hook or event receiver".into(),
            });
        }
    }

    // Preserve diagnostics for invalid plan data beyond dangling indices (for
    // example unsupported hooks, duplicate owners, or an invalid status proof).
    if !graph.transitions.is_empty() || !graph.emission_plan.is_empty() {
        projection.diagnostics.extend(
            codegen::check_plan(graph, &graph.emission_plan)
                .into_iter()
                .map(|error| FieldDiagnostic {
                    reference: FieldRef::Graph,
                    message: error.to_string(),
                }),
        );
    }
    for error in behavior::validate(graph) {
        let transition = error
            .item
            .strip_prefix("transition ")
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<usize>().ok());
        let states: Vec<_> = graph
            .states
            .iter()
            .filter(|state| {
                state.status.as_ref().is_some_and(|binding| {
                    error.item == "status bindings"
                        || transition
                            .is_some_and(|i| binding.roots.values().flatten().any(|j| *j == i))
                })
            })
            .collect();
        if states.is_empty() {
            projection.diagnostics.push(FieldDiagnostic {
                reference: FieldRef::Graph,
                message: error.to_string(),
            });
        } else {
            for state in states {
                projection.diagnostics.push(FieldDiagnostic {
                    reference: FieldRef::State(state.name.clone()),
                    message: error.to_string(),
                });
            }
        }
    }
    for state in &graph.states {
        if let Some(binding) = &state.status {
            if graph.states.iter().any(|other| {
                other.name != state.name
                    && other
                        .status
                        .as_ref()
                        .is_some_and(|b| b.target == binding.target)
            }) {
                projection.diagnostics.push(FieldDiagnostic {
                    reference: FieldRef::State(state.name.clone()),
                    message: "Duplicate status owner".into(),
                });
            }
        }
        if state
            .status
            .as_ref()
            .is_some_and(|b| b.policy == super::status_model::Policy::Owned)
            && super::status_recipes::motions(graph, &state.name)
                .0
                .is_empty()
        {
            projection.diagnostics.push(FieldDiagnostic {
                reference: FieldRef::State(state.name.clone()),
                message: "Owned status has no ground motion".into(),
            });
        }
    }

    if projection.nodes.len() > MAX_FIELD_ITEMS || projection.edges.len() > MAX_FIELD_ITEMS {
        return Err(format!(
            "field projection exceeds its item limit ({MAX_FIELD_ITEMS})"
        ));
    }
    Ok(projection)
}

/// Connect a declared state to one transition endpoint through the validated editor.
pub fn connect_state(
    graph: &BehaviorGraph,
    fighter: &str,
    state_name: &str,
    transition_index: usize,
    endpoint: TransitionEndpoint,
) -> Result<BehaviorGraph, String> {
    preflight(graph)?;
    bounded_text(fighter, live_model::MAX_FIGHTER_BYTES, "fighter name")?;
    bounded_text(state_name, live_model::MAX_NAME_BYTES, "state name")?;
    if fighter.contains('\0') || state_name.contains('\0') {
        return Err("fighter and state names must not contain NUL bytes".into());
    }
    if !graph.states.iter().any(|state| state.name == state_name) {
        return Err(format!("state {state_name:?} does not exist"));
    }
    if super::field_values::is_managed_timer_transition(graph, transition_index) {
        return Err(
            "reusable timer lifecycle endpoints are controlled by Duration and Repeat".into(),
        );
    }
    let mut transition = graph
        .transitions
        .get(transition_index)
        .cloned()
        .ok_or_else(|| format!("transition index {transition_index} is out of range"))?;
    match endpoint {
        TransitionEndpoint::From => transition.from = state_name.to_owned(),
        TransitionEndpoint::To => transition.to = state_name.to_owned(),
    }
    graph_edit::replace_transition(graph, fighter, transition_index, transition)
}

/// Connect a typed variable to a Set/Add target after checking the existing value.
///
/// Set supports Float→Float, Int→Int, and Int→Timer, matching the canonical
/// behavior validator's timer-start rule. Add supports only matching Float/Int.
pub fn connect_variable(
    graph: &BehaviorGraph,
    fighter: &str,
    variable_name: &str,
    transition_index: usize,
    action_index: usize,
) -> Result<BehaviorGraph, String> {
    preflight(graph)?;
    bounded_text(fighter, live_model::MAX_FIGHTER_BYTES, "fighter name")?;
    bounded_text(variable_name, live_model::MAX_NAME_BYTES, "variable name")?;
    if fighter.contains('\0') || variable_name.contains('\0') {
        return Err("fighter and variable names must not contain NUL bytes".into());
    }
    let variable = graph
        .vars
        .iter()
        .find(|variable| variable.name == variable_name)
        .ok_or_else(|| format!("variable {variable_name:?} does not exist"))?;
    let action = graph
        .transitions
        .get(transition_index)
        .and_then(|transition| transition.actions.get(action_index))
        .ok_or_else(|| {
            format!("action {action_index} in transition {transition_index} is out of range")
        })?;
    if is_reusable_timer_control(graph)
        && (action_target(action).is_some_and(is_managed_timer_name)
            || is_managed_timer_name(variable_name))
    {
        return Err(
            "reusable timer bookkeeping targets are controlled by Duration and Repeat".into(),
        );
    }
    let replacement = match action {
        Action::Set { value, .. } => {
            let event_scopes = behavior::EventReadScopes::checked(graph)?;
            let value_kind = expression_type(value, graph, &event_scopes, transition_index)?;
            let compatible = matches!(
                (variable.kind, value_kind),
                (VarType::Float, VarType::Float)
                    | (VarType::Int, VarType::Int)
                    | (VarType::Timer, VarType::Int)
            );
            if !compatible {
                return Err(format!(
                    "Set value type {value_kind:?} is incompatible with {:?} variable {:?}",
                    variable.kind, variable.name
                ));
            }
            Action::Set {
                var: variable.name.clone(),
                value: value.clone(),
            }
        }
        Action::Add { value, .. } => {
            let event_scopes = behavior::EventReadScopes::checked(graph)?;
            let value_kind = expression_type(value, graph, &event_scopes, transition_index)?;
            let compatible = matches!(
                (variable.kind, value_kind),
                (VarType::Float, VarType::Float) | (VarType::Int, VarType::Int)
            );
            if !compatible {
                return Err(format!(
                    "Add value type {value_kind:?} is incompatible with {:?} variable {:?}",
                    variable.kind, variable.name
                ));
            }
            Action::Add {
                var: variable.name.clone(),
                value: value.clone(),
            }
        }
        Action::SetFlag { value, .. } => {
            if variable.kind != VarType::Flag {
                return Err("Set flag requires a Flag target.".into());
            }
            Action::SetFlag {
                var: variable.name.clone(),
                value: value.clone(),
            }
        }
        _ => {
            return Err(
                "variable connections are supported only for Set, Add and Set flag actions".into(),
            )
        }
    };
    graph_edit::replace_action(graph, fighter, transition_index, action_index, replacement)
}

/// Reorder one action relative to a source field connection.
///
/// A transition source places its action target first. An action source places
/// its target immediately after it. References must stay within one transition.
/// Generated move-trigger timer bookkeeping and reusable-controller timer
/// actions cannot move or be crossed by this edit.
pub fn connect_action_sequence(
    graph: &BehaviorGraph,
    fighter: &str,
    source: &FieldRef,
    target: &FieldRef,
) -> Result<BehaviorGraph, String> {
    preflight(graph)?;
    bounded_text(fighter, live_model::MAX_FIGHTER_BYTES, "fighter name")?;
    if fighter.contains('\0') {
        return Err("fighter name must not contain a NUL byte".into());
    }

    let (transition_index, source_index) = match source {
        FieldRef::Transition(transition_index) => (*transition_index, None),
        FieldRef::Action { transition, index } => (*transition, Some(*index)),
        _ => return Err("action sequence source must be a transition or action".into()),
    };
    let (target_transition, target_index) = match target {
        FieldRef::Action { transition, index } => (*transition, *index),
        _ => return Err("action sequence target must be an action".into()),
    };
    if target_transition != transition_index {
        return Err("action sequence connections cannot cross transitions".into());
    }
    if source == target {
        return Err("an action cannot be connected to itself".into());
    }

    let transition = graph
        .transitions
        .get(transition_index)
        .ok_or_else(|| format!("transition index {transition_index} is out of range"))?;
    let action_count = transition.actions.len();
    if target_index >= action_count {
        return Err(format!(
            "target action index {target_index} is out of range"
        ));
    }
    if let Some(source_index) = source_index {
        if source_index >= action_count {
            return Err(format!(
                "source action index {source_index} is out of range"
            ));
        }
        if target_index == source_index + 1 {
            return Err("target action is already immediately after the source".into());
        }
    } else if target_index == 0 {
        return Err("target action is already first in the transition".into());
    }

    let to_index = match source_index {
        None => 0,
        Some(source_index) if target_index < source_index => source_index,
        Some(source_index) => source_index + 1,
    };
    ensure_not_reordering_timer_bookkeeping(
        transition,
        is_reusable_timer_control(graph),
        source_index,
        target_index,
        to_index,
    )?;
    graph_edit::reorder_action(graph, fighter, transition_index, target_index, to_index)
}

fn ensure_not_reordering_timer_bookkeeping(
    transition: &Transition,
    reusable_timer: bool,
    source_index: Option<usize>,
    target_index: usize,
    destination: usize,
) -> Result<(), String> {
    let protected = |index: usize| {
        transition.actions.get(index).is_some_and(|action| {
            action_target(action).is_some_and(|name| {
                name.starts_with("__mta_") || (reusable_timer && is_managed_timer_name(name))
            })
        })
    };
    if protected(target_index) || source_index.is_some_and(protected) {
        return Err("reusable timer bookkeeping actions cannot be reordered".into());
    }

    // `destination` is the insertion index after removing the target. Recover
    // the original interval so crossing a generated latch/writer marker cannot
    // change its relative execution order.
    let first = target_index.min(destination);
    let last = target_index.max(destination);
    let crosses_protected = (first..=last)
        .any(|index| index != target_index && index < transition.actions.len() && protected(index));
    if crosses_protected {
        Err("action order cannot cross reusable timer bookkeeping".into())
    } else {
        Ok(())
    }
}

fn is_reusable_timer_control(graph: &BehaviorGraph) -> bool {
    super::controller::extract_timer(graph).is_ok()
}

fn is_managed_timer_name(name: &str) -> bool {
    matches!(name, "remaining" | "ready" | "elapsed_events")
}

impl FieldRef {
    /// Canonical escaped key suitable for saved layout metadata.
    pub fn key(&self) -> String {
        match self {
            Self::Graph => "graph".into(),
            Self::State(name) => format!("state/{}", escape(name)),
            Self::Transition(index) => format!("transition/{index}"),
            Self::Action { transition, index } => format!("action/{transition}/{index}"),
            Self::Variable(name) => format!("variable/{}", escape(name)),
            Self::EventField { event, field } => {
                format!("event-field/{}/{}", escape(event), escape(field))
            }
            Self::Expression { root, path } => format!(
                "expression/{}/{}",
                value_root_key(root),
                binary_path_key(&path.0)
            ),
            Self::Predicate { transition, path } => {
                format!("predicate/{transition}/{}", binary_path_key(&path.0))
            }
            Self::SourceBinding(source) => {
                let (kind, index) = source.parts();
                format!("source/{kind}/{index}")
            }
            Self::MissingBinding { transition } => format!("missing-binding/{transition}"),
            Self::MissingTransition { source, transition } => {
                let (kind, index) = source.parts();
                format!("missing-transition/{kind}/{index}/{transition}")
            }
            Self::MissingState(name) => format!("missing-state/{}", escape(name)),
            Self::MissingVariable(name) => format!("missing-variable/{}", escape(name)),
            Self::MissingEventBinding(name) => format!("missing-event-binding/{}", escape(name)),
            Self::MissingSourceHook { source } => {
                let (kind, index) = source.parts();
                format!("missing-source-hook/{kind}/{index}")
            }
        }
    }

    /// Parse only canonical keys; alternate encodings do not alias one field item.
    pub fn from_key(key: &str) -> Result<Self, String> {
        if key.len() > MAX_KEY_BYTES {
            return Err("field key exceeds the length limit".into());
        }
        let parts = key.split('/').collect::<Vec<_>>();
        let parsed = match parts.as_slice() {
            ["graph"] => Self::Graph,
            ["state", name] => Self::State(unescape(name)?),
            ["transition", index] => Self::Transition(parse_index(index)?),
            ["action", transition, index] => Self::Action {
                transition: parse_index(transition)?,
                index: parse_index(index)?,
            },
            ["variable", name] => Self::Variable(unescape(name)?),
            ["event-field", event, field] => Self::EventField {
                event: unescape(event)?,
                field: unescape(field)?,
            },
            ["expression", "action", transition, index, path] => Self::Expression {
                root: ValueRoot::Action {
                    transition: parse_index(transition)?,
                    index: parse_index(index)?,
                },
                path: ExprPath(parse_binary_path(path)?),
            },
            ["expression", side @ ("left" | "right"), transition, predicate, expression] => {
                Self::Expression {
                    root: if *side == "left" {
                        ValueRoot::GuardLeft {
                            transition: parse_index(transition)?,
                            path: PredicatePath(parse_binary_path(predicate)?),
                        }
                    } else {
                        ValueRoot::GuardRight {
                            transition: parse_index(transition)?,
                            path: PredicatePath(parse_binary_path(predicate)?),
                        }
                    },
                    path: ExprPath(parse_binary_path(expression)?),
                }
            }
            ["predicate", transition, path] => Self::Predicate {
                transition: parse_index(transition)?,
                path: PredicatePath(parse_binary_path(path)?),
            },
            ["source", kind, index] => {
                Self::SourceBinding(SourceBindingRef::parse(kind, parse_index(index)?)?)
            }
            ["missing-binding", transition] => Self::MissingBinding {
                transition: parse_index(transition)?,
            },
            ["missing-transition", kind, binding, transition] => Self::MissingTransition {
                source: SourceBindingRef::parse(kind, parse_index(binding)?)?,
                transition: parse_index(transition)?,
            },
            ["missing-state", name] => Self::MissingState(unescape(name)?),
            ["missing-variable", name] => Self::MissingVariable(unescape(name)?),
            ["missing-event-binding", name] => Self::MissingEventBinding(unescape(name)?),
            ["missing-source-hook", kind, index] => Self::MissingSourceHook {
                source: SourceBindingRef::parse(kind, parse_index(index)?)?,
            },
            _ => return Err("unrecognized field key".into()),
        };
        if parsed.key() != key {
            return Err("field key is not in canonical form".into());
        }
        Ok(parsed)
    }
}

fn value_root_key(root: &ValueRoot) -> String {
    match root {
        ValueRoot::Action { transition, index } => format!("action/{transition}/{index}"),
        ValueRoot::GuardLeft { transition, path } => {
            format!("left/{transition}/{}", binary_path_key(&path.0))
        }
        ValueRoot::GuardRight { transition, path } => {
            format!("right/{transition}/{}", binary_path_key(&path.0))
        }
    }
}

fn binary_path_key(path: &[u8]) -> String {
    if path.is_empty() {
        "root".into()
    } else {
        path.iter().map(|side| char::from(b'0' + side)).collect()
    }
}

fn parse_binary_path(path: &str) -> Result<Vec<u8>, String> {
    if path == "root" {
        return Ok(Vec::new());
    }
    if path.is_empty()
        || path.len() > live_model::MAX_EXPR_DEPTH
        || !path.bytes().all(|byte| matches!(byte, b'0' | b'1'))
    {
        return Err("field expression path is outside binary depth bounds".into());
    }
    Ok(path.bytes().map(|byte| byte - b'0').collect())
}

impl SourceBindingRef {
    fn parts(self) -> (&'static str, usize) {
        match self {
            Self::Hook(index) => ("hook", index),
            Self::Event(index) => ("event", index),
            Self::Status(index) => ("status", index),
            Self::Resource(index) => ("resource", index),
        }
    }

    fn parse(kind: &str, index: usize) -> Result<Self, String> {
        match kind {
            "hook" => Ok(Self::Hook(index)),
            "event" => Ok(Self::Event(index)),
            "status" => Ok(Self::Status(index)),
            "resource" => Ok(Self::Resource(index)),
            _ => Err("unknown source-binding kind".into()),
        }
    }
}

fn add_source_nodes(
    projection: &mut FieldProjection,
    known: &mut HashSet<FieldRef>,
    plan: &EmissionPlan,
) {
    for index in 0..plan.bindings.len() {
        add_node(
            projection,
            known,
            FieldRef::SourceBinding(SourceBindingRef::Hook(index)),
        );
    }
    for index in 0..plan.event_bindings.len() {
        add_node(
            projection,
            known,
            FieldRef::SourceBinding(SourceBindingRef::Event(index)),
        );
    }
    for index in 0..plan.status_triggers.len() {
        add_node(
            projection,
            known,
            FieldRef::SourceBinding(SourceBindingRef::Status(index)),
        );
    }
    for index in 0..plan.resource_triggers.len() {
        add_node(
            projection,
            known,
            FieldRef::SourceBinding(SourceBindingRef::Resource(index)),
        );
    }
}

fn project_binding_edges(
    graph: &BehaviorGraph,
    projection: &mut FieldProjection,
    known: &mut HashSet<FieldRef>,
    owners: &mut [bool],
) {
    for (index, binding) in graph.emission_plan.bindings.iter().enumerate() {
        project_indices(
            projection,
            known,
            owners,
            SourceBindingRef::Hook(index),
            &binding.transitions,
            FieldEdgeKind::HookTransition,
        );
    }
    for (index, binding) in graph.emission_plan.event_bindings.iter().enumerate() {
        project_indices(
            projection,
            known,
            owners,
            SourceBindingRef::Event(index),
            &binding.transitions,
            FieldEdgeKind::EventTransition,
        );
    }
    for (index, binding) in graph.emission_plan.status_triggers.iter().enumerate() {
        project_indices(
            projection,
            known,
            owners,
            SourceBindingRef::Status(index),
            &binding.transitions,
            FieldEdgeKind::StatusTransition,
        );
    }
}

fn project_source_hooks(
    graph: &BehaviorGraph,
    projection: &mut FieldProjection,
    known: &mut HashSet<FieldRef>,
) {
    for (status_index, status) in graph.emission_plan.status_triggers.iter().enumerate() {
        let source = SourceBindingRef::Status(status_index);
        project_source_hook_relation(
            projection,
            known,
            source,
            status.hook,
            graph
                .emission_plan
                .bindings
                .iter()
                .enumerate()
                .filter_map(|(index, binding)| (binding.hook == status.hook).then_some(index)),
        );
    }
    for (resource_index, resource) in graph.emission_plan.resource_triggers.iter().enumerate() {
        let source = SourceBindingRef::Resource(resource_index);
        project_source_hook_relation(
            projection,
            known,
            source,
            resource.hook,
            graph
                .emission_plan
                .bindings
                .iter()
                .enumerate()
                .filter_map(|(index, binding)| (binding.hook == resource.hook).then_some(index)),
        );
    }
}

fn project_source_hook_relation(
    projection: &mut FieldProjection,
    known: &mut HashSet<FieldRef>,
    source: SourceBindingRef,
    hook: super::hooks::HookName,
    matching_hooks: impl Iterator<Item = usize>,
) {
    let source_ref = FieldRef::SourceBinding(source);
    let matches = matching_hooks
        .map(|index| FieldRef::SourceBinding(SourceBindingRef::Hook(index)))
        .collect::<Vec<_>>();
    if matches.is_empty() {
        let missing = FieldRef::MissingSourceHook { source };
        add_node(projection, known, missing.clone());
        projection.edges.push(FieldEdge {
            from: Some(source_ref),
            to: Some(missing.clone()),
            kind: FieldEdgeKind::SourceHook,
        });
        projection.diagnostics.push(FieldDiagnostic {
            reference: missing,
            message: format!("{} source has no matching hook binding", hook.name()),
        });
    } else {
        for target in matches {
            projection.edges.push(FieldEdge {
                from: Some(source_ref.clone()),
                to: Some(target),
                kind: FieldEdgeKind::SourceHook,
            });
        }
    }
}

fn project_indices(
    projection: &mut FieldProjection,
    known: &mut HashSet<FieldRef>,
    owners: &mut [bool],
    source: SourceBindingRef,
    indexes: &[usize],
    kind: FieldEdgeKind,
) {
    let source_ref = FieldRef::SourceBinding(source);
    if indexes.is_empty() {
        projection.diagnostics.push(FieldDiagnostic {
            reference: source_ref.clone(),
            message: "source binding selects no transitions".into(),
        });
    }
    for &index in indexes {
        if let Some(owned) = owners.get_mut(index) {
            if matches!(
                source,
                SourceBindingRef::Hook(_) | SourceBindingRef::Event(_)
            ) {
                *owned = true;
            }
            projection.edges.push(FieldEdge {
                from: Some(source_ref.clone()),
                to: Some(FieldRef::Transition(index)),
                kind,
            });
        } else {
            let missing = FieldRef::MissingTransition {
                source,
                transition: index,
            };
            add_node(projection, known, missing.clone());
            projection.edges.push(FieldEdge {
                from: Some(source_ref.clone()),
                to: Some(missing.clone()),
                kind,
            });
            projection.diagnostics.push(FieldDiagnostic {
                reference: missing,
                message: format!("source binding refers to missing transition {index}"),
            });
        }
    }
}

fn state_reference(
    graph: &BehaviorGraph,
    name: &str,
    projection: &mut FieldProjection,
    known: &mut HashSet<FieldRef>,
) -> FieldRef {
    if graph.states.iter().any(|state| state.name == name) {
        FieldRef::State(name.to_owned())
    } else {
        let missing = FieldRef::MissingState(name.to_owned());
        add_node(projection, known, missing.clone());
        projection.diagnostics.push(FieldDiagnostic {
            reference: missing.clone(),
            message: format!("state reference {name:?} is not declared"),
        });
        missing
    }
}

fn variable_reference(
    graph: &BehaviorGraph,
    name: &str,
    projection: &mut FieldProjection,
    known: &mut HashSet<FieldRef>,
) -> FieldRef {
    if graph.vars.iter().any(|variable| variable.name == name) {
        FieldRef::Variable(name.to_owned())
    } else {
        let missing = FieldRef::MissingVariable(name.to_owned());
        add_node(projection, known, missing.clone());
        projection.diagnostics.push(FieldDiagnostic {
            reference: missing.clone(),
            message: format!("variable reference {name:?} is not declared"),
        });
        missing
    }
}

fn add_node(projection: &mut FieldProjection, known: &mut HashSet<FieldRef>, reference: FieldRef) {
    if known.insert(reference.clone()) {
        projection.nodes.push(FieldNode { reference });
    }
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
        | Action::EmitEventArguments { .. }
        | Action::EmitEventValues { .. } => None,
    }
}

fn emitted_event_name(action: &Action) -> Option<&str> {
    match action {
        Action::EmitEvent { name }
        | Action::EmitEventArguments { name, .. }
        | Action::EmitEventValues { name, .. } => Some(name),
        _ => None,
    }
}

fn action_value(action: &Action) -> Option<&Expr> {
    match action {
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
            Some(value)
        }
        _ => None,
    }
}

fn expression_variable_reads(expression: &Expr) -> Vec<(&str, FieldEdgeKind)> {
    let mut output = Vec::new();
    let mut pending = vec![expression];
    while let Some(expression) = pending.pop() {
        match expression {
            Expr::StatusFrame
            | Expr::MotionFrame
            | Expr::StickX { .. }
            | Expr::StickY
            | Expr::RandomInt { .. }
            | Expr::RandomFloat { .. } => {}
            Expr::PendingDamage => {}
            Expr::Var(name) => output.push((name.as_str(), FieldEdgeKind::ActionValueRead)),
            Expr::TimerRemaining(name) => {
                output.push((name.as_str(), FieldEdgeKind::TimerRemainingRead));
            }
            Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
                pending.push(right);
                pending.push(left);
            }
            Expr::Const(_)
            | Expr::EventField { .. }
            | Expr::NativeFloat(_)
            | Expr::NativeInt(_) => {}
        }
    }
    output
}

fn predicate_variable_reads(predicate: &Predicate) -> Vec<(&str, FieldEdgeKind)> {
    let mut output = Vec::new();
    let mut pending = vec![predicate];
    while let Some(predicate) = pending.pop() {
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
                output.push((name.as_str(), FieldEdgeKind::GuardVariableRead));
            }
            Predicate::NativeFlag(_)
            | Predicate::DamageStatusIs(_)
            | Predicate::EventFlag { .. } => {}
            Predicate::Compare { left, right, .. } => {
                output.extend(
                    expression_variable_reads(left)
                        .into_iter()
                        .map(|(name, kind)| {
                            (
                                name,
                                if kind == FieldEdgeKind::TimerRemainingRead {
                                    kind
                                } else {
                                    FieldEdgeKind::GuardVariableRead
                                },
                            )
                        }),
                );
                output.extend(
                    expression_variable_reads(right)
                        .into_iter()
                        .map(|(name, kind)| {
                            (
                                name,
                                if kind == FieldEdgeKind::TimerRemainingRead {
                                    kind
                                } else {
                                    FieldEdgeKind::GuardVariableRead
                                },
                            )
                        }),
                );
            }
            Predicate::Not(inner) => pending.push(inner),
            Predicate::And(left, right) | Predicate::Or(left, right) => {
                pending.push(right);
                pending.push(left);
            }
        }
    }
    output
}

fn expression_event_fields(expression: &Expr) -> Vec<(&str, &str)> {
    let mut output = Vec::new();
    let mut pending = vec![expression];
    while let Some(expression) = pending.pop() {
        match expression {
            Expr::StatusFrame
            | Expr::MotionFrame
            | Expr::StickX { .. }
            | Expr::StickY
            | Expr::RandomInt { .. }
            | Expr::RandomFloat { .. } => {}
            Expr::EventField { event, field } => output.push((event.as_str(), field.as_str())),
            Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
                pending.push(right);
                pending.push(left);
            }
            Expr::PendingDamage
            | Expr::Const(_)
            | Expr::Var(_)
            | Expr::TimerRemaining(_)
            | Expr::NativeFloat(_)
            | Expr::NativeInt(_) => {}
        }
    }
    output
}

fn predicate_event_fields(predicate: &Predicate) -> Vec<(&str, &str)> {
    let mut output = Vec::new();
    let mut pending = vec![predicate];
    while let Some(predicate) = pending.pop() {
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
            Predicate::EventFlag { event, field } => output.push((event.as_str(), field.as_str())),
            Predicate::Compare { left, right, .. } => {
                output.extend(expression_event_fields(left));
                output.extend(expression_event_fields(right));
            }
            Predicate::Not(inner) => pending.push(inner),
            Predicate::And(left, right) | Predicate::Or(left, right) => {
                pending.push(right);
                pending.push(left);
            }
            Predicate::Flag(_)
            | Predicate::NativeFlag(_)
            | Predicate::DamageStatusIs(_)
            | Predicate::Expired(_) => {}
        }
    }
    output
}

fn expression_type(
    expression: &Expr,
    graph: &BehaviorGraph,
    event_scopes: &behavior::EventReadScopes<'_>,
    transition: usize,
) -> Result<VarType, String> {
    match expression {
        Expr::StatusFrame => Ok(VarType::Int),
        Expr::MotionFrame | Expr::StickX { .. } | Expr::StickY | Expr::RandomFloat { .. } => {
            Ok(VarType::Float)
        }
        Expr::RandomInt { .. } => Ok(VarType::Int),
        Expr::PendingDamage => Ok(VarType::Float),
        Expr::NativeFloat(_) => Ok(VarType::Float),
        Expr::NativeInt(_) => Ok(VarType::Int),
        Expr::Const(value) => Ok(match value {
            super::behavior::ConstValue::Float(_) => VarType::Float,
            super::behavior::ConstValue::Int(_) => VarType::Int,
            super::behavior::ConstValue::Flag(_) => VarType::Flag,
        }),
        Expr::Var(name) => graph
            .vars
            .iter()
            .find(|variable| variable.name == *name)
            .map(|variable| variable.kind)
            .ok_or_else(|| format!("expression variable {name:?} is not declared")),
        Expr::TimerRemaining(name) => graph
            .vars
            .iter()
            .find(|variable| variable.name == *name && variable.kind == VarType::Timer)
            .map(|_| VarType::Int)
            .ok_or_else(|| format!("timer remaining requires a declared Timer, got {name:?}")),
        Expr::EventField { event, field } => {
            let kind = event_scopes.field_kind(transition, event, field)?;
            if matches!(kind, VarType::Int | VarType::Float) {
                Ok(kind)
            } else {
                Err(format!("event field type {kind:?} is not numeric"))
            }
        }
        Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
            let left = expression_type(left, graph, event_scopes, transition)?;
            let right = expression_type(right, graph, event_scopes, transition)?;
            match (left, right) {
                (VarType::Float, VarType::Float) => Ok(VarType::Float),
                (VarType::Int, VarType::Int) => Ok(VarType::Int),
                _ => Err("expression operands do not have one matching numeric type".into()),
            }
        }
    }
}

fn preflight(graph: &BehaviorGraph) -> Result<(), String> {
    use live_model::{
        MAX_ACTIONS_PER_TRANSITION, MAX_BINDINGS, MAX_EVENT_BINDINGS, MAX_NAME_BYTES, MAX_STATES,
        MAX_SYMBOL_BYTES, MAX_TRANSITIONS, MAX_VARIABLES,
    };

    if graph.states.len() > MAX_STATES
        || graph.vars.len() > MAX_VARIABLES
        || graph.transitions.len() > MAX_TRANSITIONS
        || graph.emission_plan.bindings.len() > MAX_BINDINGS
        || graph.emission_plan.event_bindings.len() > MAX_EVENT_BINDINGS
        || graph.emission_plan.status_triggers.len() > MAX_BINDINGS
        || graph.emission_plan.resource_triggers.len() > MAX_BINDINGS
    {
        return Err("behavior exceeds field projection record limits".into());
    }
    bounded_text(&graph.initial, MAX_NAME_BYTES, "entry state")?;
    for state in &graph.states {
        bounded_text(&state.name, MAX_NAME_BYTES, "state name")?;
    }
    for variable in &graph.vars {
        bounded_text(&variable.name, MAX_NAME_BYTES, "variable name")?;
    }
    let mut source_reference_count = 0usize;
    for event in &graph.emission_plan.event_bindings {
        bounded_text(&event.name, MAX_NAME_BYTES, "event name")?;
        super::event_payload::validate_schema("field projection", &event.name, &event.fields)
            .map_err(|errors| {
                errors
                    .into_iter()
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            })?;
        bounded_indices(&event.transitions)?;
        source_reference_count = source_reference_count.saturating_add(event.transitions.len());
    }
    for binding in &graph.emission_plan.bindings {
        bounded_indices(&binding.transitions)?;
        source_reference_count = source_reference_count.saturating_add(binding.transitions.len());
    }
    for trigger in &graph.emission_plan.status_triggers {
        bounded_text(&trigger.symbol, MAX_SYMBOL_BYTES, "status symbol")?;
        bounded_text(&trigger.evidence, MAX_EVIDENCE_BYTES, "status evidence")?;
        bounded_indices(&trigger.transitions)?;
        source_reference_count = source_reference_count.saturating_add(trigger.transitions.len());
    }

    let mut expression_nodes = 0usize;
    let mut native_inputs = Vec::new();
    let mut action_count = 0usize;
    let mut emitted_event_count = 0usize;
    let mut event_argument_count = 0usize;
    for transition in &graph.transitions {
        bounded_text(&transition.from, MAX_NAME_BYTES, "transition source")?;
        bounded_text(&transition.to, MAX_NAME_BYTES, "transition target")?;
        preflight_predicate(
            &transition.guard,
            1,
            &mut expression_nodes,
            &mut native_inputs,
        )?;
        if transition.actions.len() > MAX_ACTIONS_PER_TRANSITION {
            return Err("transition exceeds the field projection action limit".into());
        }
        action_count = action_count.saturating_add(transition.actions.len());
        for action in &transition.actions {
            match action {
                Action::Status(c) => {
                    for e in c.expressions() {
                        preflight_expr(e, 1, &mut expression_nodes, &mut native_inputs)?;
                    }
                }
                Action::Object { op, .. } => {
                    for e in op.expressions() {
                        preflight_expr(e, 1, &mut expression_nodes, &mut native_inputs)?;
                    }
                }
                Action::AddSpeed { x, y } => {
                    preflight_expr(x, 1, &mut expression_nodes, &mut native_inputs)?;
                    preflight_expr(y, 1, &mut expression_nodes, &mut native_inputs)?;
                }
                Action::SetNative { value, .. } | Action::AddNative { value, .. } => {
                    preflight_expr(value, 1, &mut expression_nodes, &mut native_inputs)?
                }
                Action::SetNativeFlag { value, .. } => {
                    preflight_predicate(value, 1, &mut expression_nodes, &mut native_inputs)?
                }
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
                | Action::Resource(crate::gimmicks::resource_commands::Command {
                    amount, ..
                }) => preflight_expr(amount, 1, &mut expression_nodes, &mut native_inputs)?,
                Action::CombatPolicy { .. }
                | Action::ThrowHeldOpponent { .. }
                | Action::ReleaseHeldOpponent
                | Action::CancelIncomingHit
                | Action::Movement(_)
                | Action::Item(_)
                | Action::MeshVisibility { .. }
                | Action::ChooseHeroMenuSpell { .. } => {}
                Action::Set { var, value } | Action::Add { var, value } => {
                    bounded_text(var, MAX_NAME_BYTES, "action variable")?;
                    preflight_expr(value, 1, &mut expression_nodes, &mut native_inputs)?;
                }
                Action::SetFlag { var, value } => {
                    bounded_text(var, MAX_NAME_BYTES, "action variable")?;
                    preflight_predicate(value, 1, &mut expression_nodes, &mut native_inputs)?;
                }
                Action::Raise { var } | Action::Clear { var } | Action::Reset { var } => {
                    bounded_text(var, MAX_NAME_BYTES, "action variable")?;
                }
                Action::GenerateArticle { symbol }
                | Action::RemoveArticle { symbol }
                | Action::RemoveNewestArticle { symbol }
                | Action::ArticleMovement { symbol, .. } => {
                    bounded_text(symbol, MAX_SYMBOL_BYTES, "article symbol")?;
                }
                Action::RequestArticleStatus { symbol, status } => {
                    bounded_text(symbol, MAX_SYMBOL_BYTES, "article symbol")?;
                    bounded_text(status, MAX_SYMBOL_BYTES, "article status")?;
                }
                Action::RequestFighterStatus { symbol }
                | Action::RequestWeaponStatus { symbol, .. } => {
                    bounded_text(symbol, MAX_SYMBOL_BYTES, "weapon status symbol")?;
                }
                Action::EmitEvent { name } => {
                    bounded_text(name, MAX_NAME_BYTES, "emitted event name")?;
                    emitted_event_count = emitted_event_count.saturating_add(1);
                }
                Action::EmitEventArguments { name, arguments } => {
                    bounded_text(name, MAX_NAME_BYTES, "emitted event name")?;
                    super::event_payload::validate_argument_shape(
                        "field projection",
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
                    event_argument_count = event_argument_count.saturating_add(arguments.len());
                    emitted_event_count = emitted_event_count.saturating_add(1);
                }
                Action::EmitEventValues { name, arguments } => {
                    bounded_text(name, MAX_NAME_BYTES, "emitted event name")?;
                    if arguments.len() > super::event_payload::MAX_FIELDS_PER_EVENT {
                        return Err(format!(
                            "dynamic event argument count exceeds projection limit {}",
                            super::event_payload::MAX_FIELDS_PER_EVENT
                        ));
                    }
                    for argument in arguments {
                        bounded_text(&argument.field, MAX_NAME_BYTES, "event field name")?;
                    }
                    super::event_payload::validate_value_argument_shape(
                        "field projection",
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
                    event_argument_count = event_argument_count.saturating_add(arguments.len());
                    emitted_event_count = emitted_event_count.saturating_add(1);
                    for argument in arguments {
                        match &argument.value {
                            EventValue::Constant(_) => {}
                            EventValue::Numeric(expression) => preflight_expr(
                                expression,
                                1,
                                &mut expression_nodes,
                                &mut native_inputs,
                            )?,
                            EventValue::Flag(predicate) => preflight_predicate(
                                predicate,
                                1,
                                &mut expression_nodes,
                                &mut native_inputs,
                            )?,
                        }
                    }
                }
            }
        }
    }
    let base_items = graph
        .states
        .len()
        .saturating_add(graph.vars.len())
        .saturating_add(graph.transitions.len())
        .saturating_add(
            graph
                .transitions
                .iter()
                .map(|transition| transition.actions.len())
                .sum::<usize>(),
        )
        .saturating_add(graph.emission_plan.bindings.len())
        .saturating_add(graph.emission_plan.event_bindings.len())
        .saturating_add(graph.emission_plan.status_triggers.len())
        .saturating_add(graph.emission_plan.resource_triggers.len())
        .saturating_add(expression_nodes)
        .saturating_add(event_argument_count);
    let status_and_resource_count = graph
        .emission_plan
        .status_triggers
        .len()
        .saturating_add(graph.emission_plan.resource_triggers.len());
    let node_upper_bound = base_items
        .saturating_add(source_reference_count)
        .saturating_add(graph.transitions.len().saturating_mul(3))
        .saturating_add(action_count.saturating_mul(2))
        .saturating_add(emitted_event_count)
        .saturating_add(status_and_resource_count);
    let edge_upper_bound = graph
        .transitions
        .len()
        .saturating_mul(3)
        .saturating_add(expression_nodes.saturating_mul(2))
        .saturating_add(action_count.saturating_mul(2))
        .saturating_add(
            emitted_event_count.saturating_mul(graph.emission_plan.event_bindings.len()),
        )
        .saturating_add(graph.emission_plan.event_bindings.len())
        .saturating_add(source_reference_count)
        .saturating_add(
            status_and_resource_count.saturating_mul(graph.emission_plan.bindings.len()),
        )
        .saturating_add(1);
    if node_upper_bound > MAX_FIELD_ITEMS || edge_upper_bound > MAX_FIELD_ITEMS {
        return Err(format!(
            "behavior exceeds field projection item limit ({MAX_FIELD_ITEMS})"
        ));
    }
    Ok(())
}

fn preflight_expr(
    expression: &Expr,
    depth: usize,
    total: &mut usize,
    native_inputs: &mut Vec<super::native_inputs::NativeInput>,
) -> Result<(), String> {
    let mut pending = vec![(expression, depth)];
    while let Some((expression, depth)) = pending.pop() {
        add_expression_node(
            total,
            depth,
            live_model::MAX_EXPR_DEPTH,
            live_model::MAX_EXPR_NODES,
        )?;
        match expression {
            Expr::StatusFrame
            | Expr::MotionFrame
            | Expr::StickX { .. }
            | Expr::StickY
            | Expr::RandomInt { .. }
            | Expr::RandomFloat { .. } => {}
            Expr::PendingDamage => {}
            Expr::Var(name) | Expr::TimerRemaining(name) => {
                bounded_text(name, live_model::MAX_NAME_BYTES, "expression variable")?
            }
            Expr::EventField { event, field } => {
                bounded_text(event, live_model::MAX_NAME_BYTES, "event field event name")?;
                bounded_text(field, live_model::MAX_NAME_BYTES, "event field name")?;
            }
            Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
                pending.push((right, depth + 1));
                pending.push((left, depth + 1));
            }
            Expr::Const(_) => {}
            Expr::NativeFloat(source) | Expr::NativeInt(source) => {
                bounded_text(
                    source.symbol(),
                    super::native_inputs::NativeInput::MAX_REFERENCE_BYTES,
                    "native input source",
                )?;
                if !native_inputs.contains(source) {
                    native_inputs.push(source.clone());
                }
                if native_inputs.len() > live_model::MAX_NATIVE_INPUTS {
                    return Err(format!(
                        "native input references exceed projection limit {}",
                        live_model::MAX_NATIVE_INPUTS
                    ));
                }
            }
        }
    }
    Ok(())
}

fn preflight_predicate(
    predicate: &Predicate,
    depth: usize,
    total: &mut usize,
    native_inputs: &mut Vec<super::native_inputs::NativeInput>,
) -> Result<(), String> {
    let mut pending = vec![(predicate, depth)];
    while let Some((predicate, depth)) = pending.pop() {
        add_expression_node(
            total,
            depth,
            live_model::MAX_EXPR_DEPTH,
            live_model::MAX_EXPR_NODES,
        )?;
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
                bounded_text(name, live_model::MAX_NAME_BYTES, "guard variable")?;
            }
            Predicate::EventFlag { event, field } => {
                bounded_text(event, live_model::MAX_NAME_BYTES, "event field event name")?;
                bounded_text(field, live_model::MAX_NAME_BYTES, "event field name")?;
            }
            Predicate::DamageStatusIs(source) => {
                if !source.reference_is_bounded() {
                    return Err("contact status selector exceeds its reference bound".into());
                }
            }
            Predicate::NativeFlag(source) => {
                bounded_text(
                    source.symbol(),
                    super::native_inputs::NativeInput::MAX_REFERENCE_BYTES,
                    "native input source",
                )?;
                if !native_inputs.contains(source) {
                    native_inputs.push(source.clone());
                }
                if native_inputs.len() > live_model::MAX_NATIVE_INPUTS {
                    return Err(format!(
                        "native input references exceed projection limit {}",
                        live_model::MAX_NATIVE_INPUTS
                    ));
                }
            }
            Predicate::Compare { left, right, .. } => {
                preflight_expr(left, depth + 1, total, native_inputs)?;
                preflight_expr(right, depth + 1, total, native_inputs)?;
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

fn add_expression_node(
    total: &mut usize,
    depth: usize,
    max_depth: usize,
    max_nodes: usize,
) -> Result<(), String> {
    if depth > max_depth {
        return Err(format!(
            "expression depth exceeds projection limit {max_depth}"
        ));
    }
    *total = total.saturating_add(1);
    if *total > max_nodes {
        return Err(format!(
            "expression graph exceeds projection limit {max_nodes}"
        ));
    }
    Ok(())
}

fn bounded_indices(indices: &[usize]) -> Result<(), String> {
    if indices.len() > live_model::MAX_TRANSITIONS {
        Err("emission source exceeds the field projection transition-reference limit".into())
    } else {
        Ok(())
    }
}

fn bounded_text(text: &str, limit: usize, subject: &str) -> Result<(), String> {
    if text.len() > limit {
        Err(format!(
            "{subject} exceeds the field projection limit of {limit} bytes"
        ))
    } else {
        Ok(())
    }
}

fn parse_index(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| "field key contains an invalid index".to_string())?;
    if parsed.to_string() != value {
        return Err("field key index is not canonical".into());
    }
    Ok(parsed)
}

fn escape(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(hex_digit(byte >> 4));
            encoded.push(hex_digit(byte & 0x0f));
        }
    }
    encoded
}

fn unescape(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err("field key has an incomplete escape".into());
            }
            let high = from_hex(bytes[index + 1])?;
            let low = from_hex(bytes[index + 2])?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            if !(bytes[index].is_ascii_alphanumeric()
                || matches!(bytes[index], b'-' | b'.' | b'_' | b'~'))
            {
                return Err("field key contains an unescaped byte".into());
            }
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| "field key escape is not valid UTF-8".into())
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        _ => char::from(b'A' + value - 10),
    }
}

fn from_hex(value: u8) -> Result<u8, String> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err("field key escapes must use uppercase hexadecimal".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::{
        behavior::{CmpOp, ConstValue, StateDef, Transition, VarDef},
        codegen::{EventBinding, HookBinding, ResourceSource, ResourceTrigger, StatusTrigger},
        hooks::HookName,
    };

    fn representative_graph() -> BehaviorGraph {
        let vars = vec![
            VarDef {
                keep_across_form_switch: false,
                name: "charge meter".into(),
                kind: VarType::Float,
                initial: ConstValue::Float(0.0),
            },
            VarDef {
                keep_across_form_switch: false,
                name: "timer".into(),
                kind: VarType::Timer,
                initial: ConstValue::Int(30),
            },
            VarDef {
                keep_across_form_switch: false,
                name: "ready".into(),
                kind: VarType::Flag,
                initial: ConstValue::Flag(false),
            },
        ];
        let expression = Expr::Add(
            Box::new(Expr::Var("charge meter".into())),
            Box::new(Expr::Const(ConstValue::Float(1.0))),
        );
        let transitions = vec![
            Transition {
                from: "idle".into(),
                to: "charging".into(),
                guard: Predicate::Compare {
                    op: CmpOp::Gte,
                    left: Expr::Var("charge meter".into()),
                    right: Expr::Const(ConstValue::Float(10.0)),
                },
                on_frame: true,
                actions: vec![
                    Action::Set {
                        var: "charge meter".into(),
                        value: expression,
                    },
                    Action::Add {
                        var: "charge meter".into(),
                        value: Expr::Const(ConstValue::Float(1.0)),
                    },
                    Action::Raise {
                        var: "ready".into(),
                    },
                    Action::Clear {
                        var: "ready".into(),
                    },
                    Action::Reset {
                        var: "timer".into(),
                    },
                    Action::GenerateArticle {
                        symbol: "cloud_wave".into(),
                    },
                    Action::RemoveArticle {
                        symbol: "cloud_wave".into(),
                    },
                    Action::RequestWeaponStatus {
                        target: super::super::live_model::WeaponTarget {
                            battle_object_id: 7,
                        },
                        symbol: "weapon_status_test".into(),
                    },
                    Action::EmitEvent {
                        name: "charged".into(),
                    },
                ],
            },
            Transition {
                from: "charging".into(),
                to: "powered".into(),
                guard: Predicate::And(
                    Box::new(Predicate::Flag("ready".into())),
                    Box::new(Predicate::Expired("timer".into())),
                ),
                on_frame: false,
                actions: vec![Action::Set {
                    var: "timer".into(),
                    value: Expr::Const(ConstValue::Int(30)),
                }],
            },
            Transition {
                from: "powered".into(),
                to: "idle".into(),
                guard: Predicate::Not(Box::new(Predicate::Flag("ready".into()))),
                on_frame: true,
                actions: vec![Action::Reset {
                    var: "charge meter".into(),
                }],
            },
            Transition {
                from: "idle".into(),
                to: "idle".into(),
                guard: Predicate::Compare {
                    op: CmpOp::Eq,
                    left: Expr::Const(ConstValue::Int(0)),
                    right: Expr::Const(ConstValue::Int(0)),
                },
                on_frame: true,
                actions: vec![],
            },
        ];
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
                    name: "charging".into(),
                },
                StateDef {
                    status: None,
                    name: "powered".into(),
                },
            ],
            vars,
            transitions,
            emission_plan: EmissionPlan {
                bindings: vec![
                    HookBinding {
                        hook: HookName::OnChargeTick,
                        article: None,
                        transitions: vec![0],
                    },
                    HookBinding {
                        hook: HookName::OnSpend,
                        article: None,
                        transitions: vec![1],
                    },
                    HookBinding {
                        hook: HookName::OnStatusEnter,
                        article: None,
                        transitions: vec![2],
                    },
                    HookBinding {
                        hook: HookName::OnTimeout,
                        article: None,
                        transitions: vec![3],
                    },
                ],
                event_bindings: vec![EventBinding {
                    name: "charged".into(),
                    transitions: vec![3],
                    fields: Vec::new(),
                }],
                status_triggers: vec![
                    StatusTrigger {
                        hook: HookName::OnStatusEnter,
                        symbol: "FIGHTER_STATUS_KIND_SPECIAL_LW".into(),
                        evidence: "status edge reviewed".into(),
                        runtime_validated: true,
                        transitions: vec![2],
                    },
                    StatusTrigger {
                        hook: HookName::OnSpend,
                        symbol: "FIGHTER_STATUS_KIND_SPECIAL_N".into(),
                        evidence: "spend edge reviewed".into(),
                        runtime_validated: true,
                        transitions: vec![],
                    },
                ],
                resource_triggers: vec![ResourceTrigger {
                    hook: HookName::OnTimeout,
                    source: ResourceSource::CloudLimitIdleExpiry,
                }],
            },
        }
    }

    #[test]
    fn projection_covers_every_semantic_record_without_mutating_graph() {
        let graph = representative_graph();
        let before = graph.clone();
        let projection = project(&graph).unwrap();
        assert_eq!(graph, before);
        assert_eq!(
            projection
                .nodes
                .iter()
                .filter(|node| matches!(node.reference, FieldRef::Transition(_)))
                .count(),
            graph.transitions.len()
        );
        assert_eq!(
            projection
                .nodes
                .iter()
                .filter(|node| matches!(node.reference, FieldRef::Action { .. }))
                .count(),
            graph
                .transitions
                .iter()
                .map(|transition| transition.actions.len())
                .sum::<usize>()
        );
        assert_eq!(
            projection
                .nodes
                .iter()
                .filter(|node| matches!(node.reference, FieldRef::SourceBinding(_)))
                .count(),
            4 + 1 + 2 + 1
        );
        assert!(projection
            .edges
            .iter()
            .any(|edge| edge.kind == FieldEdgeKind::StatusTransition));
        assert!(projection
            .edges
            .iter()
            .any(|edge| edge.kind == FieldEdgeKind::EventTransition));
        assert!(projection
            .edges
            .iter()
            .any(|edge| edge.kind == FieldEdgeKind::HookTransition));
        assert!(projection
            .edges
            .iter()
            .any(|edge| edge.kind == FieldEdgeKind::ActionSequence));
        assert!(projection
            .edges
            .iter()
            .any(|edge| edge.kind == FieldEdgeKind::SourceHook));
        assert!(projection
            .edges
            .iter()
            .any(|edge| edge.kind == FieldEdgeKind::EventDispatch));
        assert!(!projection
            .nodes
            .iter()
            .any(|node| matches!(node.reference, FieldRef::MissingBinding { .. })));
    }

    fn editable_graph() -> BehaviorGraph {
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
            vars: vec![
                VarDef {
                    keep_across_form_switch: false,
                    name: "left".into(),
                    kind: VarType::Float,
                    initial: ConstValue::Float(0.0),
                },
                VarDef {
                    keep_across_form_switch: false,
                    name: "right".into(),
                    kind: VarType::Float,
                    initial: ConstValue::Float(0.0),
                },
                VarDef {
                    keep_across_form_switch: false,
                    name: "flag".into(),
                    kind: VarType::Flag,
                    initial: ConstValue::Flag(false),
                },
                VarDef {
                    keep_across_form_switch: false,
                    name: "timer".into(),
                    kind: VarType::Timer,
                    initial: ConstValue::Int(30),
                },
            ],
            transitions: vec![
                Transition {
                    from: "idle".into(),
                    to: "active".into(),
                    guard: Predicate::Compare {
                        op: CmpOp::Gte,
                        left: Expr::Var("left".into()),
                        right: Expr::Const(ConstValue::Float(1.0)),
                    },
                    on_frame: true,
                    actions: vec![
                        Action::Set {
                            var: "left".into(),
                            value: Expr::Var("left".into()),
                        },
                        Action::EmitEvent {
                            name: "ready".into(),
                        },
                        Action::RequestWeaponStatus {
                            target: super::super::live_model::WeaponTarget {
                                battle_object_id: 0x2000_0001,
                            },
                            symbol: "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR".into(),
                        },
                    ],
                },
                Transition {
                    from: "active".into(),
                    to: "idle".into(),
                    guard: Predicate::Flag("flag".into()),
                    on_frame: true,
                    actions: vec![Action::Reset { var: "left".into() }],
                },
                Transition {
                    from: "active".into(),
                    to: "active".into(),
                    guard: Predicate::Expired("timer".into()),
                    on_frame: true,
                    actions: vec![Action::Set {
                        var: "timer".into(),
                        value: Expr::Const(ConstValue::Int(30)),
                    }],
                },
                Transition {
                    from: "idle".into(),
                    to: "idle".into(),
                    guard: Predicate::Flag("flag".into()),
                    on_frame: true,
                    actions: vec![Action::Add {
                        var: "left".into(),
                        value: Expr::Const(ConstValue::Float(1.0)),
                    }],
                },
            ],
            emission_plan: EmissionPlan {
                bindings: vec![
                    HookBinding {
                        hook: HookName::OnChargeTick,
                        article: None,
                        transitions: vec![0],
                    },
                    HookBinding {
                        hook: HookName::OnStatusEnter,
                        article: None,
                        transitions: vec![1],
                    },
                    HookBinding {
                        hook: HookName::OnSpend,
                        article: None,
                        transitions: vec![2],
                    },
                ],
                event_bindings: vec![EventBinding {
                    name: "ready".into(),
                    transitions: vec![3],
                    fields: Vec::new(),
                }],
                status_triggers: vec![StatusTrigger {
                    hook: HookName::OnStatusEnter,
                    symbol: "FIGHTER_STATUS_KIND_SPECIAL_LW".into(),
                    evidence: "reviewed status edge evidence".into(),
                    runtime_validated: true,
                    transitions: vec![1],
                }],
                resource_triggers: vec![ResourceTrigger {
                    hook: HookName::OnSpend,
                    source: ResourceSource::CloudLimitGroundedNeutral,
                }],
            },
        }
    }

    #[test]
    fn validated_connection_edits_preserve_full_emission_plan() {
        let graph = editable_graph();
        let plan = graph.emission_plan.clone();
        let connected =
            connect_state(&graph, "cloud", "idle", 1, TransitionEndpoint::From).unwrap();
        assert_eq!(connected.transitions[1].from, "idle");
        assert_eq!(connected.emission_plan, plan);

        let connected = connect_variable(&connected, "cloud", "right", 0, 0).unwrap();
        assert!(matches!(
            &connected.transitions[0].actions[0],
            Action::Set { var, .. } if var == "right"
        ));
        assert_eq!(connected.emission_plan, plan);
    }

    #[test]
    fn invalid_connections_reject_atomically() {
        let graph = editable_graph();
        let before = graph.clone();
        assert!(connect_state(&graph, "cloud", "missing", 0, TransitionEndpoint::To).is_err());
        assert!(connect_state(&graph, "cloud", "active", 99, TransitionEndpoint::To).is_err());
        assert!(connect_variable(&graph, "cloud", "flag", 0, 0).is_err());
        assert!(connect_variable(&graph, "cloud", "right", 0, 99).is_err());
        assert_eq!(graph, before);
    }

    #[test]
    fn variable_connections_preserve_reusable_timer_duration_and_repeat_controls() {
        let graph = super::super::templates::Preset::Timer.graph();
        assert!(super::super::controller::extract_timer(&graph).is_ok());
        let (transition_index, action_index) = graph
            .transitions
            .iter()
            .enumerate()
            .find_map(|(transition_index, transition)| {
                transition
                    .actions
                    .iter()
                    .enumerate()
                    .find_map(|(action_index, action)| {
                        matches!(
                            action,
                            Action::Set {
                                var,
                                value: Expr::Const(ConstValue::Int(_))
                            } if var == "remaining"
                        )
                        .then_some((transition_index, action_index))
                    })
            })
            .expect("reusable timer fixture writes its controlled duration");
        let before = graph.clone();

        // Both variables are integer-compatible with this Set expression, but
        // redirecting a controller-owned target would break Duration/Repeat.
        assert!(connect_variable(
            &graph,
            "mario",
            "elapsed_events",
            transition_index,
            action_index,
        )
        .is_err());
        assert!(connect_action_sequence(
            &graph,
            "mario",
            &FieldRef::Transition(transition_index),
            &FieldRef::Action {
                transition: transition_index,
                index: action_index,
            },
        )
        .is_err());
        assert_eq!(graph, before);
        assert!(super::super::controller::extract_timer(&graph).is_ok());
    }
}
