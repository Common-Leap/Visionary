//! Validate controller IR before static or live compilation.
use super::{
    behavior::{self, Action, BehaviorGraph, VarType},
    codegen,
};
use crate::move_nodes::model::NodeError;
use std::collections::BTreeMap;
pub(crate) fn compile(
    fighter: &str,
    base: &BehaviorGraph,
) -> Result<BehaviorGraph, Vec<NodeError>> {
    preflight_base_event_headers(fighter, base)?;
    super::field_projection::project(base)
        .map_err(|e| vec![node_error(fighter, "base", "graph", e)])?;
    let mut schemas = BTreeMap::new();
    for binding in &base.emission_plan.event_bindings {
        super::event_payload::validate_schema("base", &binding.name, &binding.fields).map_err(
            |e| {
                e.into_iter()
                    .map(|e| node_error(fighter, "base", "event binding", e.to_string()))
                    .collect::<Vec<_>>()
            },
        )?;
        if schemas
            .insert(binding.name.clone(), binding.fields.clone())
            .is_some()
        {
            return Err(vec![node_error(
                fighter,
                "base",
                "event binding",
                "base behavior declares a named-event receiver more than once",
            )]);
        }
    }
    for (index, transition) in base.transitions.iter().enumerate() {
        validate_event_sends(
            fighter,
            "base behavior",
            &format!("transition {index}"),
            &transition.actions,
            &schemas,
        )?;
    }
    validate_result(fighter, base.clone(), &BTreeMap::new())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::{
        behavior::{CmpOp, ConstValue, Expr, Predicate, StateDef, Transition, VarType},
        codegen::{EmissionPlan, EventBinding},
    };
    use crate::move_nodes::test_support as fixture;
    fn timer_base() -> BehaviorGraph {
        BehaviorGraph {
            initial: "idle".into(),
            states: vec![StateDef {
                status: None,
                name: "idle".into(),
            }],
            vars: vec![fixture::var(
                "duration".into(),
                VarType::Timer,
                ConstValue::Int(0),
            )],
            ..BehaviorGraph::default()
        }
    }
    #[test]
    fn oversized_base_expression_is_rejected_by_bounded_preflight() {
        let deep = (0..=crate::gimmicks::live_model::MAX_EXPR_DEPTH)
            .fold(Expr::Const(ConstValue::Int(0)), |inner, _| {
                Expr::Add(Box::new(inner), Box::new(Expr::Const(ConstValue::Int(1))))
            });
        let base = BehaviorGraph {
            initial: "idle".into(),
            states: vec![StateDef {
                status: None,
                name: "idle".into(),
            }],
            transitions: vec![Transition {
                from: "idle".into(),
                to: "idle".into(),
                guard: Predicate::Compare {
                    op: CmpOp::Gt,
                    left: deep,
                    right: Expr::Const(ConstValue::Int(0)),
                },
                on_frame: false,
                actions: vec![],
            }],
            ..BehaviorGraph::default()
        };
        let error = compile("mario", &base).unwrap_err();
        assert!(error
            .iter()
            .any(|item| item.subject == "mario/base/graph" && item.reason.contains("depth")));
    }
    #[test]
    fn base_schema_validation_runs_without_move_receivers_and_is_failure_atomic() {
        let nine_fields = (0..9)
            .map(|index| crate::gimmicks::event_payload::FieldDef {
                name: format!("field_{index}"),
                kind: VarType::Int,
            })
            .collect::<Vec<_>>();
        for (fields, expected_reason) in [
            (nine_fields, "maximum is 8"),
            (
                vec![crate::gimmicks::event_payload::FieldDef {
                    name: "timer".into(),
                    kind: VarType::Timer,
                }],
                "Timer is not a supported event field kind",
            ),
            (
                vec![crate::gimmicks::event_payload::FieldDef {
                    name: "bad\0field".into(),
                    kind: VarType::Int,
                }],
                "must not contain NUL",
            ),
        ] {
            let base = BehaviorGraph {
                emission_plan: EmissionPlan {
                    event_bindings: vec![EventBinding {
                        name: "external".into(),
                        transitions: vec![],
                        fields,
                    }],
                    ..EmissionPlan::default()
                },
                ..timer_base()
            };
            let before = serde_json::to_value(&base).unwrap();
            let errors = compile("fighter", &base).unwrap_err();
            assert_eq!(serde_json::to_value(&base).unwrap(), before);
            assert_eq!(errors.len(), 1, "expected one bounded base-schema error");
            assert!(errors[0].reason.contains(expected_reason));
        }
    }
    #[test]
    fn duplicate_base_event_bindings_are_rejected_even_without_move_receivers() {
        let base = BehaviorGraph {
            emission_plan: EmissionPlan {
                event_bindings: vec![
                    EventBinding {
                        name: "external".into(),
                        transitions: vec![],
                        fields: vec![],
                    },
                    EventBinding {
                        name: "external".into(),
                        transitions: vec![],
                        fields: vec![],
                    },
                ],
                ..EmissionPlan::default()
            },
            ..timer_base()
        };
        let before = serde_json::to_value(&base).unwrap();
        let errors = compile("fighter", &base).unwrap_err();
        assert_eq!(serde_json::to_value(&base).unwrap(), before);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].reason.contains("more than once"));
    }
    #[test]
    fn base_event_count_and_names_are_checked_before_schema_traversal() {
        let binding = |name: String| EventBinding {
            name,
            transitions: vec![],
            fields: vec![],
        };
        let mut base = timer_base();
        base.emission_plan.event_bindings = (0..=codegen::MAX_EVENT_BINDINGS)
            .map(|index| binding(format!("event_{index}")))
            .collect();
        let before = serde_json::to_value(&base).unwrap();
        let errors = compile("fighter", &base).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].reason.contains("event bindings; maximum"));
        assert_eq!(serde_json::to_value(&base).unwrap(), before);
        for name in [" ".into(), "bad\0event".into(), "é".repeat(25)] {
            base.emission_plan.event_bindings = vec![binding(name)];
            let before = serde_json::to_value(&base).unwrap();
            let errors = compile("fighter", &base).unwrap_err();
            assert_eq!(errors.len(), 1);
            assert!(errors[0].reason.contains("base event name"));
            assert_eq!(serde_json::to_value(&base).unwrap(), before);
        }
    }
}
