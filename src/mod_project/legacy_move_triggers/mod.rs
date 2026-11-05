//! Read old per-move behavior once, folding it into the main character controller.
//! Nothing in this module is part of the editable or saved move model.
mod expand;
mod lower;
mod model;
mod references;

use super::{FighterMod, ModProjectFile, RosterMod};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};

#[derive(Deserialize)]
pub(super) struct Project {
    version: u32,
    name: String,
    #[serde(default)]
    fighters: HashMap<String, serde_json::Value>,
    #[serde(default)]
    roster: RosterMod,
}

impl TryFrom<Project> for ModProjectFile {
    type Error = String;
    fn try_from(project: Project) -> Result<Self, String> {
        let mut fighters = HashMap::new();
        for (name, mut value) in project.fighters {
            let moves = value
                .as_object_mut()
                .ok_or("fighter must be an object")?
                .remove("move_graphs");
            let mut fighter: FighterMod =
                serde_json::from_value(value).map_err(|e| e.to_string())?;
            let graphs: BTreeMap<String, model::MoveGraph> = moves
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            let has_legacy = graphs.values().any(|graph| {
                !graph.variables.is_empty()
                    || !graph.variable_scopes.is_empty()
                    || !graph.triggers.is_empty()
                    || !graph.routines.is_empty()
                    || !graph.routine_inputs.is_empty()
                    || !graph.routine_outputs.is_empty()
                    || !graph.routine_calls.is_empty()
                    || !graph.routine_calls_in_routines.is_empty()
            });
            if has_legacy {
                fighter.behavior =
                    lower::compile(&name, &fighter.behavior, &graphs).map_err(|errors| {
                        errors
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join("; ")
                    })?;
            }
            for (move_name, graph) in graphs {
                let graph = crate::move_nodes::model::MoveGraph {
                    version: graph.version,
                    tracks: graph.tracks,
                    positions: graph
                        .positions
                        .into_iter()
                        .filter(|(key, _)| !has_legacy || key.starts_with("node:"))
                        .collect(),
                };
                fighter.move_graphs.insert(move_name, graph);
            }
            fighters.insert(name, fighter);
        }
        Ok(Self {
            version: project.version,
            name: project.name,
            fighters,
            roster: project.roster,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::{behavior::BehaviorGraph, live_model::Value};

    const LEGACY: &str = include_str!("../../../tests/data/controller-migration/project.json");
    // Captured with the original move_nodes::triggers::compile before removing it.
    const EXPECTED: &str = include_str!("../../../tests/data/controller-migration/main.json");

    fn legacy() -> serde_json::Value {
        serde_json::from_str(LEGACY).unwrap()
    }

    #[test]
    fn legacy_event_timer_and_routine_lower_exactly_once_into_main() {
        let mut project: ModProjectFile = serde_json::from_str(LEGACY).unwrap();
        project.migrate().unwrap();
        let fighter = &project.fighters["mario"];
        let expected: BehaviorGraph = serde_json::from_str(EXPECTED).unwrap();
        assert_eq!(fighter.behavior, expected);
        let programs = crate::gimmicks::controller_project::compile(
            "mario",
            &fighter.behavior,
            &fighter.controller_charts,
        )
        .unwrap();
        assert_eq!(programs.len(), 1);
        assert_eq!(programs[0].chart_id, "main");
        assert_eq!(programs[0].program.id, "live:mario");
        crate::move_nodes::project::expand(&project).unwrap();
        let saved = serde_json::to_value(&project).unwrap();
        let graph = &saved["fighters"]["mario"]["move_graphs"]["attack11"];
        for removed in [
            "variables",
            "variable_scopes",
            "triggers",
            "routines",
            "routine_inputs",
            "routine_outputs",
            "routine_calls",
            "routine_calls_in_routines",
        ] {
            assert!(graph.get(removed).is_none(), "saved legacy field {removed}");
        }
        let reloaded: ModProjectFile = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(serde_json::to_value(reloaded).unwrap(), saved);

        let program = &programs[0].program;
        let mut state = program.initial_state();
        program.step_event(&mut state, "begin").unwrap();
        assert_eq!(state.values["counter"], Value::Int(2));
        for _ in 0..8 {
            program.step_frame(&mut state).unwrap();
        }
        assert_eq!(state.values["counter"], Value::Int(3));
        program.step_event(&mut state, "begin").unwrap();
        for _ in 0..8 {
            program.step_frame(&mut state).unwrap();
        }
        assert_eq!(state.values["counter"], Value::Int(6));
    }

    #[test]
    fn migration_shares_equal_declarations_and_keeps_move_local_timers_independent() {
        let mut input = legacy();
        let mut sibling = input["fighters"]["mario"]["move_graphs"]["attack11"].clone();
        sibling["triggers"][0]["name"] = "sibling_begin".into();
        sibling["triggers"][1]["name"] = "sibling_finish".into();
        sibling["routine_calls"] =
            serde_json::json!({"sibling_begin":[{"routine":"increment","before":0}]});
        input["fighters"]["mario"]["move_graphs"]["attack12"] = sibling;
        let project: ModProjectFile = serde_json::from_value(input).unwrap();
        let graph = &project.fighters["mario"].behavior;
        assert_eq!(graph.vars.iter().filter(|v| v.name == "counter").count(), 1);
        let timers: Vec<_> = graph
            .vars
            .iter()
            .filter(|v| v.kind == crate::gimmicks::behavior::VarType::Timer)
            .collect();
        assert_eq!(timers.len(), 2);
        assert_ne!(timers[0].name, timers[1].name);
        let program = crate::gimmicks::live_protocol::program_from_graph("mario", graph).unwrap();
        let mut state = program.initial_state();
        program.step_event(&mut state, "begin").unwrap();
        assert_eq!(state.values["counter"], Value::Int(4));
        for _ in 0..8 {
            program.step_frame(&mut state).unwrap();
        }
        assert_eq!(state.values["counter"], Value::Int(6));
    }

    #[test]
    fn malformed_legacy_routine_and_conflicting_variables_refuse_to_load() {
        let mut missing = legacy();
        missing["fighters"]["mario"]["move_graphs"]["attack11"]["routines"] = serde_json::json!({});
        assert!(serde_json::from_value::<ModProjectFile>(missing)
            .unwrap_err()
            .to_string()
            .contains("missing routine"));
        let mut conflict = legacy();
        conflict["fighters"]["mario"]["move_graphs"]["attack11"]["variables"][0]["initial"] =
            serde_json::json!({"Int":99});
        assert!(serde_json::from_value::<ModProjectFile>(conflict)
            .unwrap_err()
            .to_string()
            .contains("conflicts"));
    }
    #[test]
    fn nested_typed_routine_ports_capture_event_fields_and_copy_outputs_inline() {
        use crate::gimmicks::behavior::{Action, Expr, Predicate};
        let mut input = legacy();
        let graph = &mut input["fighters"]["mario"]["move_graphs"]["attack11"];
        graph["triggers"][0]["source"]["event"]["fields"] = serde_json::json!([
            {"name":"delta","kind":"Int"},{"name":"enabled","kind":"Flag"}
        ]);
        graph["triggers"][0]["guard"] = serde_json::to_value(Predicate::EventFlag {
            event: "begin".into(),
            field: "enabled".into(),
        })
        .unwrap();
        let formal = references::routine_input_reference("step").unwrap();
        let result = references::routine_output_reference("result").unwrap();
        graph["routines"] = serde_json::json!({"increment":[],"inner":[serde_json::to_value(Action::Set {
            var:result.clone(),value:Expr::Add(Box::new(Expr::Var("counter".into())),Box::new(Expr::Var(formal.clone())))
        }).unwrap()]});
        graph["routine_inputs"] = serde_json::json!({"increment":[{"name":"step","kind":"Int"}],"inner":[{"name":"step","kind":"Int"}]});
        graph["routine_outputs"] = serde_json::json!({"increment":[{"name":"result","kind":"Int"}],"inner":[{"name":"result","kind":"Int"}]});
        graph["routine_calls"]["begin_cooldown"][0]["arguments"] = serde_json::json!([
            {"input":"step","value":serde_json::to_value(Expr::EventField {event:"begin".into(),field:"delta".into()}).unwrap()}
        ]);
        graph["routine_calls"]["begin_cooldown"][0]["outputs"] =
            serde_json::json!([{"output":"result","destination":"counter"}]);
        graph["routine_calls_in_routines"] = serde_json::json!({"increment":[{
            "routine":"inner","before":0,"arguments":[{"input":"step","value":serde_json::to_value(Expr::Var(formal)).unwrap()}],
            "outputs":[{"output":"result","destination":result}]
        }]});
        let project: ModProjectFile = serde_json::from_value(input).unwrap();
        let main = &project.fighters["mario"].behavior;
        let program = crate::gimmicks::controller_project::compile("mario", main, &BTreeMap::new())
            .unwrap()
            .remove(0)
            .program;
        let mut state = program.initial_state();
        let snapshot = crate::gimmicks::native_inputs::NativeSnapshot::empty();
        program
            .step_event_values_with_snapshot(
                &mut state,
                "begin",
                &[("delta", Value::Int(7)), ("enabled", Value::Flag(false))],
                &snapshot,
            )
            .unwrap();
        assert_eq!(state.values["counter"], Value::Int(0));
        program
            .step_event_values_with_snapshot(
                &mut state,
                "begin",
                &[("delta", Value::Int(7)), ("enabled", Value::Flag(true))],
                &snapshot,
            )
            .unwrap();
        assert_eq!(state.values["counter"], Value::Int(7));
        program
            .step_event_values_with_snapshot(
                &mut state,
                "begin",
                &[("delta", Value::Int(2)), ("enabled", Value::Flag(true))],
                &snapshot,
            )
            .unwrap();
        assert_eq!(state.values["counter"], Value::Int(9));
        assert!(main
            .vars
            .iter()
            .all(|var| !var.name.starts_with("__ri_") && !var.name.starts_with("__ro_")));
    }
    #[test]
    fn migrated_values_and_main_receivers_survive_move_scoping() {
        use crate::move_nodes::model::{
            CharacterValue, MoveCondition, MoveNode, NativeComparison, NodeKind,
        };
        let mut project: ModProjectFile = serde_json::from_str(LEGACY).unwrap();
        let fighter = project.fighters.get_mut("mario").unwrap();
        let timer = fighter
            .behavior
            .vars
            .iter()
            .find(|v| v.kind == crate::gimmicks::behavior::VarType::Timer)
            .unwrap()
            .name
            .clone();
        for (id, var) in [(2, "counter".to_string()), (3, timer)] {
            fighter.move_graphs.get_mut("attack11").unwrap().tracks[0]
                .nodes
                .push(MoveNode {
                    id,
                    kind: NodeKind::Branch {
                        condition: MoveCondition::CharacterValue {
                            chart_id: "main".into(),
                            var,
                            op: NativeComparison::Gte,
                            value: CharacterValue::Int(0),
                        },
                        then_nodes: vec![],
                        else_nodes: vec![],
                    },
                });
        }
        fighter
            .move_graphs
            .insert("attack12".into(), fighter.move_graphs["attack11"].clone());
        let expected = fighter.behavior.clone();
        let scoped = crate::mod_project::scope_acmd_project(
            &project,
            crate::mod_project::AcmdProjectScope::Move {
                fighter: "mario",
                move_name: "attack11",
            },
        );
        assert_eq!(scoped.fighters["mario"].move_graphs.len(), 1);
        assert_eq!(scoped.fighters["mario"].behavior, expected);
        crate::move_nodes::project::expand(&scoped).unwrap();
    }
}
