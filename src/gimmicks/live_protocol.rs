//! Desktop conversion and bounded versioned transport codec for live behavior.
//!
//! The plugin consumes only [`live_model::LiveProgram`]. Project graph types
//! stay on the desktop side of this module and are converted before encoding.

use serde::{Deserialize, Serialize};

use super::{behavior, codegen, hooks, live_model as model, registry};

pub const MAX_REQUEST_BYTES: usize = model::MAX_SNAPSHOT_BYTES;
pub const MAX_PROGRAMS: usize = 96;
pub const MAX_DISABLED_FIGHTERS: usize = 96;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveRequest {
    pub version: u8,
    /// Monotonic desktop snapshot revision. The complete program list replaces
    /// the previous set only after every item has passed validation.
    pub revision: u64,
    /// Owners explicitly removed from the project. They stay overridden as
    /// empty until a later snapshot either restores a program or live preview
    /// is intentionally cleared with an empty request.
    #[serde(default)]
    pub disabled_fighters: Vec<String>,
    pub programs: Vec<model::LiveProgram>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveEnvelope {
    #[serde(rename = "gimmick_behavior")]
    pub gimmick_behavior: LiveRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ack {
    pub version: u8,
    pub family: String,
    pub revision: u64,
    pub accepted: bool,
    pub message: String,
}

pub fn encode_request(request: &LiveRequest) -> Result<String, String> {
    if (request.version != model::MODEL_VERSION
        && !(request.version == 4
            && request
                .programs
                .iter()
                .all(|p| p.status_bindings.is_empty())))
        || request.programs.len() > MAX_PROGRAMS
        || request.disabled_fighters.len() > MAX_DISABLED_FIGHTERS
    {
        return Err("invalid live behavior request version or program count".into());
    }
    validate_snapshot(&request.programs, &request.disabled_fighters)?;
    let text = serde_json::to_string(&LiveEnvelope {
        gimmick_behavior: request.clone(),
    })
    .map_err(|error| format!("cannot encode live behavior request: {error}"))?;
    if text.len() > MAX_REQUEST_BYTES {
        return Err(format!(
            "encoded live behavior request exceeds {MAX_REQUEST_BYTES} bytes"
        ));
    }
    Ok(text)
}

pub fn validate_snapshot(
    programs: &[model::LiveProgram],
    disabled_fighters: &[String],
) -> Result<(), String> {
    if programs.len() > MAX_PROGRAMS || disabled_fighters.len() > MAX_DISABLED_FIGHTERS {
        return Err("snapshot exceeds the fighter program or disable list limit".into());
    }
    let mut fighters = std::collections::BTreeSet::new();
    let mut ids = std::collections::BTreeSet::new();
    for (index, program) in programs.iter().enumerate() {
        if !fighters.insert(program.fighter.as_str()) {
            return Err(format!(
                "programs[{index}] duplicates fighter {:?}",
                program.fighter
            ));
        }
        if !ids.insert(program.id.as_str()) {
            return Err(format!("programs[{index}] duplicates id {:?}", program.id));
        }
        if registry::fighter_kind_symbol(&program.fighter).is_none() {
            return Err(format!(
                "programs[{index}] targets an unregistered fighter {:?}",
                program.fighter
            ));
        }
        program.validate().map_err(|errors| {
            errors
                .into_iter()
                .map(|error| format!("programs[{index}] {}: {}", error.item, error.reason))
                .collect::<Vec<_>>()
                .join("; ")
        })?;
        for trigger in &program.status_triggers {
            registry::resolve(
                &program.fighter,
                &trigger.symbol,
                registry::IdentifierKind::FighterStatus,
            )
            .map_err(|error| {
                format!(
                    "programs[{index}] status trigger {}: {error}",
                    trigger.hook.wire_name()
                )
            })?;
        }
        for transition in &program.transitions {
            for action in &transition.actions {
                match action {
                    model::Action::GenerateArticle { symbol }
                    | model::Action::RemoveArticle { symbol }
                    | model::Action::RemoveNewestArticle { symbol } => {
                        registry::resolve(
                            &program.fighter,
                            symbol,
                            registry::IdentifierKind::Article,
                        )
                        .map_err(|error| format!("programs[{index}] article action: {error}"))?;
                    }
                    model::Action::RequestArticleStatus { symbol, status } => {
                        registry::resolve(
                            &program.fighter,
                            symbol,
                            registry::IdentifierKind::Article,
                        )
                        .map_err(|error| error.to_string())?;
                        let entry = registry::resolve(
                            &program.fighter,
                            status,
                            registry::IdentifierKind::WeaponStatus,
                        )
                        .map_err(|error| error.to_string())?;
                        if registry::weapon_kind(entry.fighter, entry.group).is_none() {
                            return Err("article status has no weapon-kind guard".into());
                        }
                    }
                    model::Action::RequestFighterStatus { symbol } => {
                        registry::resolve(
                            &program.fighter,
                            symbol,
                            registry::IdentifierKind::FighterStatus,
                        )
                        .map_err(|error| format!("fighter status action: {error}"))?;
                    }
                    model::Action::RequestWeaponStatus { target, symbol } => {
                        if target.battle_object_id == 0 {
                            return Err(format!("programs[{index}] weapon status target is zero"));
                        }
                        let entry = registry::resolve(
                            &program.fighter,
                            symbol,
                            registry::IdentifierKind::WeaponStatus,
                        )
                        .map_err(|error| {
                            format!("programs[{index}] weapon status action: {error}")
                        })?;
                        if registry::weapon_kind(entry.fighter, entry.group).is_none() {
                            return Err(format!(
                                "programs[{index}] weapon status has no concrete kind guard"
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    for (index, fighter) in disabled_fighters.iter().enumerate() {
        if !fighters.insert(fighter.as_str()) {
            return Err(format!(
                "disabled_fighters[{index}] overlaps or duplicates a program fighter"
            ));
        }
        if registry::fighter_kind_symbol(fighter).is_none() {
            return Err(format!(
                "disabled_fighters[{index}] names an unregistered fighter {fighter:?}"
            ));
        }
    }
    Ok(())
}

/// Convenience conversion for live authoring: the reviewed emission plan is
/// taken directly from the saved graph, with a stable per-fighter id.
pub fn program_from_graph(
    fighter: &str,
    graph: &behavior::BehaviorGraph,
) -> Result<model::LiveProgram, String> {
    from_graph(
        format!("live:{fighter}"),
        fighter,
        graph,
        &graph.emission_plan,
    )
    .map_err(|errors| errors.join("; "))
}

/// Convert an authored graph and its reviewed plan to the serde-only live DTO.
pub fn from_graph(
    id: impl Into<String>,
    fighter: &str,
    graph: &behavior::BehaviorGraph,
    plan: &codegen::EmissionPlan,
) -> Result<model::LiveProgram, Vec<String>> {
    let mut errors = behavior::validate_for_fighter(fighter, graph)
        .into_iter()
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    if !graph.is_empty() || !plan.is_empty() {
        errors.extend(
            codegen::check_plan(graph, plan)
                .into_iter()
                .map(|error| error.to_string()),
        );
        errors.extend(
            codegen::check_status_triggers(fighter, plan)
                .into_iter()
                .map(|error| error.to_string()),
        );
    }
    if registry::fighter_kind_symbol(fighter).is_none() {
        errors.push(format!(
            "fighter {fighter:?} has no exact pinned fighter-kind mapping"
        ));
    }
    // Preserve the error-only boundary: do not clone graph strings or build
    // even a partial DTO when desktop validation has already failed.
    if !errors.is_empty() {
        return Err(errors);
    }

    lower_validated_graph(id.into(), fighter, graph, plan)
}

// The availability gate stays in `from_graph`. Offline integration fixtures
// use the same lowering after validating their graph without granting export.
fn lower_validated_graph(
    id: String,
    fighter: &str,
    graph: &behavior::BehaviorGraph,
    plan: &codegen::EmissionPlan,
) -> Result<model::LiveProgram, Vec<String>> {
    let mut errors = Vec::new();
    let mut converted_actions = Vec::with_capacity(graph.transitions.len());
    for (transition_index, transition) in graph.transitions.iter().enumerate() {
        let mut actions = Vec::with_capacity(transition.actions.len());
        for (action_index, action) in transition.actions.iter().enumerate() {
            match convert_action(action, &plan.event_bindings, graph, transition_index) {
                Ok(action) => actions.push(action),
                Err(reason) => errors.push(format!(
                    "transition {transition_index} action {action_index}: {reason}"
                )),
            }
        }
        converted_actions.push(actions);
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let program = model::LiveProgram {
        custom_statuses: graph.custom_statuses.clone(),
        status_bindings: graph
            .states
            .iter()
            .filter_map(|state| {
                state
                    .status
                    .clone()
                    .map(|binding| super::status_model::StateStatus {
                        state: state.name.clone(),
                        binding,
                    })
            })
            .collect(),
        version: model::MODEL_VERSION,
        id,
        fighter: fighter.into(),
        initial: graph.initial.clone(),
        states: graph
            .states
            .iter()
            .map(|state| state.name.clone())
            .collect(),
        vars: graph.vars.clone(),
        native_inputs: behavior::collect_native_inputs(graph),
        transitions: graph
            .transitions
            .iter()
            .zip(converted_actions)
            .map(|(transition, actions)| model::Transition {
                actions,
                ..transition.clone()
            })
            .collect(),
        bindings: plan
            .bindings
            .iter()
            .filter_map(|binding| {
                Some(model::HookBinding {
                    hook: convert_hook(binding.hook)?,
                    article: binding.article.clone(),
                    transitions: binding.transitions.clone(),
                })
            })
            .collect(),
        event_bindings: plan.event_bindings.clone(),
        status_triggers: plan
            .status_triggers
            .iter()
            .filter_map(|trigger| {
                Some(model::StatusTrigger {
                    hook: convert_hook(trigger.hook)?,
                    symbol: trigger.symbol.clone(),
                    transitions: trigger.transitions.clone(),
                })
            })
            .collect(),
        resource_triggers: plan
            .resource_triggers
            .iter()
            .filter_map(|trigger| {
                Some(model::ResourceTrigger {
                    hook: convert_hook(trigger.hook)?,
                    source: match trigger.source {
                        codegen::ResourceSource::CloudLimitGroundedNeutral => {
                            model::ResourceSource::CloudLimitGroundedNeutral
                        }
                        codegen::ResourceSource::CloudLimitIdleExpiry => {
                            model::ResourceSource::CloudLimitIdleExpiry
                        }
                    },
                })
            })
            .collect(),
    };
    if let Err(model_errors) = program.validate() {
        errors.extend(model_errors.into_iter().map(|error| error.to_string()));
    }
    if errors.is_empty() {
        Ok(program)
    } else {
        Err(errors)
    }
}

#[cfg(test)]
pub(super) fn offline_program_from_graph(
    fighter: &str,
    graph: &behavior::BehaviorGraph,
) -> Result<model::LiveProgram, Vec<String>> {
    let errors: Vec<_> = behavior::validate_for_fighter(fighter, graph)
        .into_iter()
        .map(|error| error.to_string())
        .collect();
    if !errors.is_empty() {
        return Err(errors);
    }
    lower_validated_graph(
        format!("offline:{fighter}"),
        fighter,
        graph,
        &graph.emission_plan,
    )
}

fn convert_hook(hook: hooks::HookName) -> Option<model::HookName> {
    Some(match hook {
        hooks::HookName::OnDealDamage => model::HookName::OnDealDamage,
        hooks::HookName::OnTakeDamage => model::HookName::OnTakeDamage,
        hooks::HookName::OnAttackShielded => model::HookName::OnAttackShielded,
        hooks::HookName::OnShieldHit => model::HookName::OnShieldHit,
        hooks::HookName::OnIncomingHit => model::HookName::OnIncomingHit,
        hooks::HookName::OnReflect => model::HookName::OnReflect,
        hooks::HookName::OnAbsorb => model::HookName::OnAbsorb,
        hooks::HookName::OnGrab => model::HookName::OnGrab,
        hooks::HookName::OnItemPickup => model::HookName::OnItemPickup,
        hooks::HookName::OnItemThrown => model::HookName::OnItemThrown,
        hooks::HookName::OnThrow => model::HookName::OnThrow,
        hooks::HookName::OnGrabbed => model::HookName::OnGrabbed,
        hooks::HookName::OnChargeTick => model::HookName::OnChargeTick,
        hooks::HookName::OnTimeout => model::HookName::OnTimeout,
        hooks::HookName::OnSpend => model::HookName::OnSpend,
        hooks::HookName::OnStatusEnter => model::HookName::OnStatusEnter,
        hooks::HookName::OnStatusExit => model::HookName::OnStatusExit,
        hooks::HookName::OnSwitch => model::HookName::OnSwitch,
        hooks::HookName::OnStockStart => model::HookName::OnStockStart,
        hooks::HookName::OnKo => model::HookName::OnKo,
        hooks::HookName::OnArticleSpawn => model::HookName::OnArticleSpawn,
        hooks::HookName::OnArticleRemoved => model::HookName::OnArticleRemoved,
        _ => return None,
    })
}

/// Saved actions are already wire actions; event sends are checked against
/// their receiver and their arguments put into receiver declaration order.
pub(crate) fn convert_action(
    action: &behavior::Action,
    event_bindings: &[codegen::EventBinding],
    graph: &behavior::BehaviorGraph,
    transition_index: usize,
) -> Result<model::Action, String> {
    let (name, kind) = match action {
        behavior::Action::EmitEvent { name } => (name, "name-only event send"),
        behavior::Action::EmitEventArguments { name, .. } => (name, "typed event send"),
        behavior::Action::EmitEventValues { name, .. } => (name, "dynamic event send"),
        _ => return Ok(action.clone()),
    };
    let binding = event_bindings
        .iter()
        .find(|binding| binding.name == *name)
        .ok_or_else(|| format!("{kind} has no declared receiver"))?;
    let joined = |errors: Vec<super::event_payload::ValidationError>| {
        errors
            .into_iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("; ")
    };
    Ok(match action {
        behavior::Action::EmitEventArguments { arguments, .. } => {
            if binding.fields.is_empty() {
                return Err("typed event arguments require a nonempty receiver schema".into());
            }
            let values = super::event_payload::canonicalize_arguments(
                "live conversion",
                name,
                &binding.fields,
                arguments,
            )
            .map_err(joined)?;
            let arguments = binding
                .fields
                .iter()
                .zip(values)
                .map(|(field, value)| model::EventArgument {
                    field: field.name.clone(),
                    value,
                })
                .collect();
            model::Action::EmitEventArguments {
                name: name.clone(),
                arguments,
            }
        }
        behavior::Action::EmitEventValues { arguments, .. } => {
            if binding.fields.is_empty() {
                return Err("dynamic event values require a nonempty receiver schema".into());
            }
            let ordered = super::event_payload::canonicalize_value_arguments(
                "live conversion",
                name,
                &binding.fields,
                arguments,
                |value| behavior::event_value_kind(graph, transition_index, value),
            )
            .map_err(joined)?;
            let arguments = ordered.into_iter().cloned().collect();
            model::Action::EmitEventValues {
                name: name.clone(),
                arguments,
            }
        }
        _ => {
            if !binding.fields.is_empty() {
                return Err("name-only event send cannot target a receiver with fields".into());
            }
            action.clone()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_comparisons_save_in_project_spelling_and_load_the_old_wire_spelling() {
        for (saved, wire) in [("Timer", "timer"), ("Flag", "flag")] {
            let kind: model::VarKind = serde_json::from_str(&format!("{wire:?}")).unwrap();
            assert_eq!(kind, serde_json::from_str(&format!("{saved:?}")).unwrap());
            assert_eq!(serde_json::to_string(&kind).unwrap(), format!("{saved:?}"));
        }
        let op: model::CmpOp = serde_json::from_str(r#""gte""#).unwrap();
        assert_eq!(
            (op, serde_json::to_string(&op).unwrap().as_str()),
            (model::CmpOp::Gte, r#""Gte""#)
        );
    }

    #[test]
    fn form_variable_opt_in_round_trips_and_defaults_off() {
        let raw = r#"{"name":"count","kind":"Int","initial":{"Int":2}}"#;
        let mut variable: behavior::VarDef = serde_json::from_str(raw).unwrap();
        assert!(!variable.keep_across_form_switch);
        variable.keep_across_form_switch = true;
        let restored: behavior::VarDef =
            serde_json::from_str(&serde_json::to_string(&variable).unwrap()).unwrap();
        assert_eq!(variable, restored);
    }

    #[test]
    fn computed_flag_assignment_executes_like_inline_actions_after_live_conversion() {
        let mut graph = behavior::BehaviorGraph {
            initial: "active".into(),
            states: vec![behavior::StateDef {
                status: None,
                name: "active".into(),
            }],
            vars: vec![
                behavior::VarDef {
                    keep_across_form_switch: false,
                    name: "count".into(),
                    kind: behavior::VarType::Int,
                    initial: behavior::ConstValue::Int(3),
                },
                behavior::VarDef {
                    keep_across_form_switch: false,
                    name: "enabled".into(),
                    kind: behavior::VarType::Flag,
                    initial: behavior::ConstValue::Flag(false),
                },
            ],
            ..behavior::BehaviorGraph::default()
        };
        graph.transitions.push(behavior::Transition {
            from: "active".into(),
            to: "active".into(),
            guard: behavior::Predicate::Compare {
                op: behavior::CmpOp::Eq,
                left: behavior::Expr::Const(behavior::ConstValue::Int(1)),
                right: behavior::Expr::Const(behavior::ConstValue::Int(1)),
            },
            on_frame: true,
            actions: vec![behavior::Action::SetFlag {
                var: "enabled".into(),
                value: behavior::Predicate::Compare {
                    op: behavior::CmpOp::Gte,
                    left: behavior::Expr::Var("count".into()),
                    right: behavior::Expr::Const(behavior::ConstValue::Int(3)),
                },
            }],
        });
        let plan = codegen::EmissionPlan {
            bindings: vec![codegen::HookBinding {
                hook: hooks::HookName::OnStockStart,
                article: None,
                transitions: vec![0],
            }],
            ..codegen::EmissionPlan::default()
        };
        let converted = from_graph("flag_assignment", "cloud", &graph, &plan).unwrap();
        let mut converted_state = converted.initial_state();
        converted
            .step(&mut converted_state, model::HookName::OnStockStart)
            .unwrap();

        let mut inline = converted;
        inline.transitions[0].actions = vec![model::Action::SetFlag {
            var: "enabled".into(),
            value: model::Predicate::Compare {
                op: model::CmpOp::Gte,
                left: model::Expr::Var("count".into()),
                right: model::Expr::Const(model::Value::Int(3)),
            },
        }];
        let mut inline_state = inline.initial_state();
        inline
            .step(&mut inline_state, model::HookName::OnStockStart)
            .unwrap();
        assert_eq!(converted_state, inline_state);
        assert_eq!(converted_state.values["enabled"], model::Value::Flag(true));
    }
}
