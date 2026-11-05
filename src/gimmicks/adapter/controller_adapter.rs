//! Static multi-chart artifact preparation shared with the project compiler.

use std::collections::BTreeMap;

use super::{AdapterError, BehaviorGraph};
use crate::gimmicks::{controller_project, controller_protocol, registry};
use crate::mod_project::ControllerChart;

#[derive(Debug)]
pub(super) struct CompiledBundle {
    pub encoded: String,
    pub owners: Vec<ControllerOwner>,
    pub status_definitions: String,
    pub status_install: String,
}

#[derive(Debug)]
pub(super) struct ControllerOwner {
    pub fighter: String,
    pub kind_symbol: String,
    pub has_main: bool,
}

/// Compile every chart owner only after preflighting the entire candidate.
/// The shared protocol codec supplies ordering, semantic checks and byte caps.
pub(super) fn compile_bundle(
    graphs: &BTreeMap<String, BehaviorGraph>,
    controllers: &BTreeMap<String, BTreeMap<String, ControllerChart>>,
) -> Result<Option<CompiledBundle>, Vec<AdapterError>> {
    let empty_charts = BTreeMap::new();
    let mut all_controllers: BTreeMap<_, _> = controllers.iter().collect();
    for (fighter, graph) in graphs {
        if super::super::behavior::requires_controller_runtime(graph) {
            all_controllers.entry(fighter).or_insert(&empty_charts);
        }
    }
    let chart_owners = all_controllers
        .into_iter()
        .filter(|(fighter, charts)| {
            !charts.is_empty()
                || graphs
                    .get(*fighter)
                    .is_some_and(super::super::behavior::requires_controller_runtime)
        })
        .take(controller_protocol::MAX_FIGHTER_OWNERS + 1)
        .collect::<Vec<_>>();
    if chart_owners.is_empty() {
        return Ok(None);
    }
    if chart_owners.len() > controller_protocol::MAX_FIGHTER_OWNERS {
        return Err(vec![error(
            "project",
            format!(
                "static controller bundle exceeds {} owners",
                controller_protocol::MAX_FIGHTER_OWNERS
            ),
        )]);
    }
    let chart_count = chart_owners
        .iter()
        .fold(0usize, |total, (fighter, charts)| {
            total
                .saturating_add(charts.len())
                .saturating_add(usize::from(
                    graphs.get(*fighter).is_some_and(|graph| !graph.is_empty()),
                ))
        });
    if chart_count > controller_protocol::MAX_CHARTS {
        return Err(vec![error(
            "project",
            format!(
                "static controller bundle exceeds {} charts",
                controller_protocol::MAX_CHARTS
            ),
        )]);
    }

    let empty = BehaviorGraph::default();
    for (fighter, charts) in &chart_owners {
        if fighter.len() > super::super::live_model::MAX_FIGHTER_BYTES
            || fighter.trim().is_empty()
            || fighter.as_bytes().contains(&0)
        {
            return Err(vec![error(
                "fighter owner",
                format!(
                    "owner key must be nonblank, at most {} bytes, and contain no NUL",
                    super::super::live_model::MAX_FIGHTER_BYTES
                ),
            )]);
        }
        let main = graphs.get(*fighter).unwrap_or(&empty);
        super::super::behavior::preflight_bounds(main)
            .map_err(|reason| vec![error(&format!("fighter {fighter} main"), reason)])?;
        controller_project::preflight(main, charts)
            .map_err(|reason| vec![error(&format!("fighter {fighter}"), reason)])?;
    }

    let mut records = Vec::new();
    let mut owners = Vec::with_capacity(chart_owners.len());
    for (fighter, charts) in chart_owners {
        let main = graphs.get(fighter).unwrap_or(&empty);
        let compiled = controller_project::compile(fighter, main, charts)
            .map_err(|reason| vec![error(&format!("fighter {fighter}"), reason)])?;
        records.extend(compiled);
        let Some(kind) = registry::FIGHTER_KINDS
            .iter()
            .find(|entry| entry.fighter == fighter.as_str())
        else {
            return Err(vec![error(
                &format!("fighter {fighter}"),
                "no exact pinned fighter-kind mapping exists".to_string(),
            )]);
        };
        owners.push(ControllerOwner {
            fighter: fighter.clone(),
            kind_symbol: kind.symbol.to_string(),
            has_main: !main.is_empty(),
        });
    }
    let request = controller_protocol::ControllerBundleRequest {
        version: controller_protocol::REQUEST_VERSION,
        revision: 0,
        disabled_fighters: Vec::new(),
        charts: records,
    };
    let encoded = controller_protocol::encode_request(&request)
        .map_err(|reason| vec![error("project / controller bundle", reason)])?;
    owners.sort_by(|left, right| left.fighter.cmp(&right.fighter));
    let mut status_definitions = String::new();
    let mut status_install = String::new();
    let manifest =
        crate::gimmicks::status_model::manifest(request.charts.iter().map(|c| c.program.clone()))
            .map_err(|reason| vec![error("status installation", reason)])?;
    for (index, (fighter, status, _)) in manifest.iter().enumerate() {
        status_install.push_str(&format!("    smashline::Agent::new({fighter:?})"));
        for (phase, line) in [
            ("Pre", "Pre"),
            ("Init", "Init"),
            ("MainEnter", "Main"),
            ("Exec", "Exec"),
            ("End", "End"),
            ("Exit", "Exit"),
        ] {
            let thunk = format!("visionary_status_{index}_{}", phase.to_ascii_lowercase());
            status_definitions.push_str(&format!("unsafe extern \"C\" fn {thunk}(fighter: &mut smash::lua2cpp::L2CFighterCommon) -> smash::lib::L2CValue {{ super::status_runtime::dispatch(fighter, {status}i32, super::status_model::Phase::{phase}) }}\n"));
            status_install.push_str(&format!(".status(smashline::{line}, {status}i32, {thunk})"));
        }
        status_install.push_str(".install();\n");
    }
    Ok(Some(CompiledBundle {
        encoded,
        owners,
        status_definitions,
        status_install,
    }))
}

pub(super) fn dispatch_arm(owner: &ControllerOwner) -> String {
    let missing_main = if owner.has_main {
        "compiled controller main did not accept this named event"
    } else {
        "compiled controller bundle has no main chart for this fighter"
    };
    format!(
        "    if crate::slight::agents::lookup(boid).is_some_and(|record| record.category == 0 && record.kind == *{})\n        && super::live_behavior::actor_matches(boid, {:?}) {{\n        if let Some(result) = super::live_behavior::dispatch_named(boid, {:?}, name) {{ return result; }}\n        return Err({:?}.to_string());\n    }}\n",
        owner.kind_symbol, owner.fighter, owner.fighter, missing_main
    )
}

fn error(subject: &str, reason: String) -> AdapterError {
    AdapterError {
        subject: subject.to_string(),
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::{field_canvas::FieldLayout, meter, templates::Preset};

    fn timer_chart() -> ControllerChart {
        ControllerChart {
            graph: Preset::Timer.graph(),
            layout: FieldLayout::default(),
        }
    }

    fn chart_sets(pairs: &[(&str, &str)]) -> BTreeMap<String, BTreeMap<String, ControllerChart>> {
        pairs
            .iter()
            .fold(BTreeMap::new(), |mut sets, (fighter, id)| {
                sets.entry((*fighter).to_string())
                    .or_insert_with(BTreeMap::new)
                    .insert((*id).to_string(), timer_chart());
                sets
            })
    }

    #[test]
    fn no_extra_chart_entry_matches_existing_meter_renderer() {
        let graphs = BTreeMap::from([("mario".to_string(), Preset::Timer.graph())]);
        let legacy = super::super::render_with_meters(&graphs, &BTreeMap::new()).unwrap();
        let extended =
            super::super::render_with_controllers(&graphs, &BTreeMap::new(), &BTreeMap::new())
                .unwrap();
        assert_eq!(extended, legacy);
        assert!(!extended.source.contains("STATIC_CONTROLLER_BUNDLE"));
    }

    #[test]
    fn charted_owner_has_one_bundle_and_no_legacy_program_or_registration() {
        let graphs = BTreeMap::from([("mario".to_string(), Preset::Timer.graph())]);
        let controllers = chart_sets(&[("mario", "charge")]);
        let artifact =
            super::super::render_with_controllers(&graphs, &controllers, &BTreeMap::new()).unwrap();
        assert!(artifact
            .source
            .contains("visionary.gimmicks.static-controller-bundle.v1"));
        assert!(artifact
            .source
            .contains("install_compiled_controllers(VISIONARY_STATIC_CONTROLLER_BUNDLE)"));
        assert!(!artifact.source.contains("GIMMICK_MARIO_PROGRAM"));
        assert!(!artifact.source.contains("GIMMICK_MARIO_REGISTRATIONS"));
        assert!(artifact
            .source
            .contains("dispatch_named(boid, \"mario\", name)"));
        let parsed = syn::parse_file(&artifact.source).unwrap();
        assert!(!parsed.items.is_empty());
        let bundle = compile_bundle(&graphs, &controllers).unwrap().unwrap();
        let decoded = controller_protocol::decode_request(&bundle.encoded).unwrap();
        assert_eq!(decoded.revision, 0);
        assert_eq!(
            decoded
                .charts
                .iter()
                .map(|chart| chart.chart_id.as_str())
                .collect::<Vec<_>>(),
            ["charge", "main"]
        );
    }

    #[test]
    fn chart_only_owner_routes_main_exactly_and_never_falls_through() {
        let graphs = BTreeMap::new();
        let controllers = chart_sets(&[("mario", "charge")]);
        let artifact =
            super::super::render_with_controllers(&graphs, &controllers, &BTreeMap::new()).unwrap();
        let bundle = compile_bundle(&graphs, &controllers).unwrap().unwrap();
        let decoded = controller_protocol::decode_request(&bundle.encoded).unwrap();
        assert_eq!(decoded.charts.len(), 1);
        assert_eq!(decoded.charts[0].chart_id, "charge");
        assert!(!artifact.source.contains("GIMMICK_MARIO_PROGRAM"));
        let arm = dispatch_arm(&bundle.owners[0]);
        assert!(arm.contains("compiled controller bundle has no main chart"));
        assert!(arm.contains("dispatch_named(boid, \"mario\", name)"));
        assert!(!arm.contains("execute_named"));
    }

    #[test]
    fn mixed_charted_and_legacy_owners_keep_legacy_handlers_and_meters() {
        let graphs = BTreeMap::from([
            ("cloud".to_string(), Preset::Timer.graph()),
            ("mario".to_string(), Preset::Timer.graph()),
        ]);
        let controllers = chart_sets(&[("mario", "charge")]);
        let meters = BTreeMap::from([(
            "cloud".to_string(),
            vec![meter::MeterBinding {
                fighter: "cloud".into(),
                enabled: true,
                style: meter::MeterStyle::Bar,
                value: meter::MeterValue::Float {
                    symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".into(),
                    min: 0.0,
                    max: 100.0,
                },
            }],
        )]);
        let artifact =
            super::super::render_with_controllers(&graphs, &controllers, &meters).unwrap();
        assert!(artifact.source.contains("GIMMICK_CLOUD_PROGRAM"));
        assert!(artifact.source.contains("GIMMICK_CLOUD_REGISTRATIONS"));
        assert!(!artifact.source.contains("GIMMICK_MARIO_PROGRAM"));
        assert!(artifact.source.contains("meter.cloud.0"));
        assert_eq!(artifact.fighters, ["cloud", "mario"]);
        syn::parse_file(&artifact.source).unwrap();
    }

    #[test]
    fn bundle_encoding_and_generated_source_are_deterministic() {
        let graphs = BTreeMap::from([
            ("cloud".to_string(), Preset::Timer.graph()),
            ("mario".to_string(), Preset::Timer.graph()),
        ]);
        let controllers = chart_sets(&[("mario", "zeta"), ("mario", "alpha"), ("cloud", "extra")]);
        let first =
            super::super::render_with_controllers(&graphs, &controllers, &BTreeMap::new()).unwrap();
        let second =
            super::super::render_with_controllers(&graphs, &controllers, &BTreeMap::new()).unwrap();
        assert_eq!(first, second);
        let bundle = compile_bundle(&graphs, &controllers).unwrap().unwrap();
        let decoded = controller_protocol::decode_request(&bundle.encoded).unwrap();
        let order = decoded
            .charts
            .iter()
            .map(|chart| (chart.program.fighter.as_str(), chart.chart_id.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            order,
            [
                ("cloud", "extra"),
                ("cloud", "main"),
                ("mario", "alpha"),
                ("mario", "main"),
                ("mario", "zeta")
            ]
        );
    }

    #[test]
    fn invalid_owner_and_invalid_graph_fail_closed_before_rendering() {
        let unknown = chart_sets(&[("Unknown Fighter", "extra")]);
        assert!(compile_bundle(&BTreeMap::new(), &unknown).unwrap_err()[0]
            .reason
            .contains("pinned fighter-kind"));

        let invalid = BTreeMap::from([(
            "mario".into(),
            BTreeMap::from([("extra".into(), ControllerChart::default())]),
        )]);
        assert!(compile_bundle(&BTreeMap::new(), &invalid).is_err());
    }

    #[test]
    fn per_owner_and_project_chart_caps_are_checked_before_lowering() {
        let nine = (0..9)
            .map(|index| (format!("c{index}"), timer_chart()))
            .collect::<BTreeMap<_, _>>();
        let over_owner_cap = BTreeMap::from([("mario".to_string(), nine)]);
        assert!(
            compile_bundle(&BTreeMap::new(), &over_owner_cap).unwrap_err()[0]
                .reason
                .contains("exceeds 8 charts")
        );

        let mut too_many_owners = registry::FIGHTER_KINDS
            .iter()
            .map(|entry| {
                (
                    entry.fighter.to_string(),
                    BTreeMap::from([("extra".into(), timer_chart())]),
                )
            })
            .collect::<BTreeMap<_, _>>();
        while too_many_owners.len() <= controller_protocol::MAX_FIGHTER_OWNERS {
            too_many_owners.insert(
                format!("unregistered_owner_{}", too_many_owners.len()),
                BTreeMap::from([("extra".into(), timer_chart())]),
            );
        }
        assert!(
            compile_bundle(&BTreeMap::new(), &too_many_owners).unwrap_err()[0]
                .reason
                .contains("owners")
        );
    }

    #[test]
    fn aggregate_chart_and_input_variable_limits_fail_closed() {
        let mut controllers = BTreeMap::new();
        for owner in registry::FIGHTER_KINDS.iter().take(33) {
            controllers.insert(
                owner.fighter.to_string(),
                (0..8)
                    .map(|index| (format!("chart_{index}"), timer_chart()))
                    .collect(),
            );
        }
        // This exceeds the project-wide 256-chart limit while each owner is
        // within the per-fighter limit, so failure must come from shared bounds.
        assert!(
            compile_bundle(&BTreeMap::new(), &controllers).unwrap_err()[0]
                .reason
                .contains("charts")
        );

        let mut oversized = Preset::Timer.graph();
        oversized
            .vars
            .extend((0..1_800).map(|index| crate::gimmicks::behavior::VarDef {
                keep_across_form_switch: false,
                name: format!("padding_{index:04}_{}", "x".repeat(100)),
                kind: crate::gimmicks::behavior::VarType::Int,
                initial: crate::gimmicks::behavior::ConstValue::Int(0),
            }));
        let huge = BTreeMap::from([(
            "mario".into(),
            BTreeMap::from([(
                "extra".into(),
                ControllerChart {
                    graph: oversized,
                    layout: FieldLayout::default(),
                },
            )]),
        )]);
        let error = compile_bundle(&BTreeMap::new(), &huge).unwrap_err();
        assert_eq!(
            error[0].reason,
            "controller \"extra\": behavior graph exceeds a bounded table limit"
        );
    }

    #[test]
    fn aggregate_encoded_byte_cap_is_checked_by_shared_codec() {
        let controllers = registry::FIGHTER_KINDS
            .iter()
            .take(32)
            .map(|owner| {
                (
                    owner.fighter.to_string(),
                    (0..8)
                        .map(|index| (format!("chart_{index}"), timer_chart()))
                        .collect(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let error = compile_bundle(&BTreeMap::new(), &controllers).unwrap_err();
        assert!(error[0].reason.contains("bytes"), "{}", error[0]);
    }

    #[test]
    fn later_main_bounds_fail_before_any_owner_lowering() {
        let mut semantically_invalid_early_main = Preset::Timer.graph();
        semantically_invalid_early_main.initial = "missing_state".into();
        let mut oversized_later_main = Preset::Timer.graph();
        oversized_later_main
            .vars
            .extend((0..80).map(|index| crate::gimmicks::behavior::VarDef {
                keep_across_form_switch: false,
                name: format!("oversized_{index}"),
                kind: crate::gimmicks::behavior::VarType::Int,
                initial: crate::gimmicks::behavior::ConstValue::Int(0),
            }));
        let graphs = BTreeMap::from([
            ("mario".into(), semantically_invalid_early_main),
            ("samus".into(), oversized_later_main),
        ]);
        let controllers = chart_sets(&[("mario", "extra"), ("samus", "extra")]);
        let errors = compile_bundle(&graphs, &controllers).unwrap_err();
        assert!(errors[0].subject.contains("samus main"), "{}", errors[0]);
        assert_eq!(
            errors[0].reason,
            "behavior graph exceeds a bounded table limit"
        );
    }

    #[test]
    fn unknown_owner_name_is_bounded_before_it_enters_diagnostics() {
        let long_owner = "x".repeat(crate::gimmicks::live_model::MAX_FIGHTER_BYTES + 1);
        let controllers = BTreeMap::from([(
            long_owner.clone(),
            BTreeMap::from([("extra".into(), timer_chart())]),
        )]);
        let errors = compile_bundle(&BTreeMap::new(), &controllers).unwrap_err();
        assert_eq!(errors[0].subject, "fighter owner");
        assert!(!errors[0].to_string().contains(&long_owner));
        assert!(errors[0].reason.contains("48 bytes"));
    }

    #[test]
    fn static_controller_sdk_contract_source_can_be_emitted() {
        let graphs = BTreeMap::from([
            ("mario".to_string(), Preset::Timer.graph()),
            ("pikachu".to_string(), Preset::Timer.graph()),
        ]);
        let mut controllers = chart_sets(&[
            ("cloud", "extra_only"),
            ("mario", "charge"),
            ("mario", "cooldown"),
        ]);
        controllers
            .get_mut("mario")
            .unwrap()
            .get_mut("charge")
            .unwrap()
            .graph
            .transitions[0]
            .guard = crate::gimmicks::behavior::Predicate::And(
            Box::new(crate::gimmicks::behavior::Predicate::NativeFlag(
                crate::gimmicks::native_inputs::NativeInput::AttackResultFlag {
                    fighter: "mario".into(),
                    query: crate::gimmicks::native_inputs::AttackResultQuery::CurrentStatus,
                    mask: crate::gimmicks::native_inputs::AttackResultMask::Hit,
                },
            )),
            Box::new(crate::gimmicks::behavior::Predicate::NativeFlag(
                crate::gimmicks::native_inputs::NativeInput::ArticleExists {
                    fighter: "mario".into(),
                    symbol: "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL".into(),
                },
            )),
        );
        let meters = BTreeMap::from([(
            "cloud".to_string(),
            vec![meter::MeterBinding {
                fighter: "cloud".into(),
                enabled: true,
                style: meter::MeterStyle::Bar,
                value: meter::MeterValue::Float {
                    symbol: "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE".into(),
                    min: 0.0,
                    max: 100.0,
                },
            }],
        )]);
        let mut artifact =
            super::super::render_with_controllers(&graphs, &controllers, &meters).unwrap();
        let chart_values_script = crate::data::AcmdScript {
            stmts: vec![crate::data::AcmdStmt::Excute(vec![
                crate::data::ExcuteStmt::GimmickChartEventValues {
                    chart_id: "right".into(),
                    name: "pulse".into(),
                    arguments: vec![
                        crate::gimmicks::event_payload::Argument {
                            field: "amount".into(),
                            value: crate::gimmicks::behavior::ConstValue::Int(3),
                        },
                        crate::gimmicks::event_payload::Argument {
                            field: "gain".into(),
                            value: crate::gimmicks::behavior::ConstValue::Float(0.5),
                        },
                        crate::gimmicks::event_payload::Argument {
                            field: "on".into(),
                            value: crate::gimmicks::behavior::ConstValue::Flag(true),
                        },
                    ],
                },
            ])],
        };
        let chart_values_fn =
            crate::acmd::preview_game_fn(&chart_values_script, "sdk_chart_values_contract");
        assert!(chart_values_fn.contains("dispatch_named_chart_values_from_module"));
        assert!(chart_values_fn.contains("ChartEventValue::Int(3)"));
        assert!(chart_values_fn.contains("ChartEventValue::Float(0.5f32)"));
        assert!(chart_values_fn.contains("ChartEventValue::Flag(true)"));
        artifact.source.push_str(
            "\nmod visionary_chart_values_sdk_contract {\n    use smash::lua2cpp::L2CAgentBase;\n    use smash_script::macros;\n\n",
        );
        for line in chart_values_fn.lines() {
            artifact.source.push_str("    ");
            artifact.source.push_str(line);
            artifact.source.push('\n');
        }
        artifact.source.push_str("}\n");
        assert!(artifact
            .source
            .contains("VISIONARY_STATIC_CONTROLLER_BUNDLE_CONTRACT"));
        assert!(artifact.source.contains(
            "dispatch_named_chart_values_from_module(agent.module_accessor, \"right\", \"pulse\""
        ));
        assert!(artifact.source.contains("GIMMICK_PIKACHU_PROGRAM"));
        syn::parse_file(&artifact.source).unwrap();
        if let Some(destination) = std::env::var_os("VISIONARY_STATIC_CONTROLLER_CONTRACT_OUT") {
            super::super::write_to(std::path::Path::new(&destination), &artifact)
                .expect("cache harness selected a writable Rust destination");
        }
    }

    /// The `steer_projectile` chart of fixtures/mario-batch4-combined.json, beside its
    /// `combat_scopes` chart, packaged exactly as `--gimmick-package` emits it and
    /// decoded the way the plugin installs it. Three self-loop frame rules: steer while
    /// alive, remember facing while not, remove on a guard press while alive.
    #[test]
    fn emitted_self_loop_frame_rules_all_run_in_binding_order() {
        use crate::gimmicks::live_model::{Action, Expr, Value};
        use crate::gimmicks::native_inputs::*;
        let alive = serde_json::json!({"NativeFlag": "owned_object:mario:FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL:alive"});
        let fireball = "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL";
        let rule = |guard: serde_json::Value, action: serde_json::Value| serde_json::json!({"from": "Steer", "to": "Steer", "on_frame": true, "guard": guard, "actions": [action]});
        let stick =
            |axis: serde_json::Value| serde_json::json!({"Mul": [axis, {"Const": {"Float": 2.0}}]});
        let charts: BTreeMap<String, ControllerChart> = serde_json::from_value(serde_json::json!({
            "combat_scopes": {"graph": {
                "initial": "Start", "states": [{"name": "Start"}, {"name": "Armored"}, {"name": "Normal"}], "vars": [],
                "transitions": [
                    {"from": "Start", "to": "Armored", "on_frame": true, "guard": {"Compare": {"op": "Eq", "left": {"Const": {"Int": 0}}, "right": {"Const": {"Int": 0}}}},
                     "actions": [{"CombatPolicy": {"scope": "UntilCleared", "op": "SuperArmor"}}]},
                    {"from": "Armored", "to": "Normal", "on_frame": true, "guard": {"Button": {"button": "special", "edge": "Pressed"}},
                     "actions": [{"CombatPolicy": {"scope": "Status", "op": {"Clear": "Armor"}}}]},
                ],
                "emission_plan": {"bindings": [{"hook": "onChargeTick", "transitions": [0, 1]}], "event_bindings": []},
            }},
            "steer_projectile": {"graph": {
                "initial": "Steer", "states": [{"name": "Steer"}],
                "vars": [{"name": "launch_dir", "kind": "Float", "initial": {"Float": 1.0}}],
                "transitions": [
                    rule(alive.clone(), serde_json::json!({"Object": {"symbol": fireball, "op": {"SetSpeed": {
                        "x": {"Add": [{"Var": "launch_dir"}, stick(serde_json::json!({"StickX": {"relative_to_facing": false}}))]},
                        "y": stick(serde_json::json!("StickY"))}}}})),
                    rule(serde_json::json!({"Not": alive}), serde_json::json!({"Set": {"var": "launch_dir", "value": {"NativeFloat": "facing:mario"}}})),
                    rule(serde_json::json!({"And": [alive, {"Button": {"button": "guard", "edge": "Pressed"}}]}), serde_json::json!({"Object": {"symbol": fireball, "op": "Remove"}})),
                ],
                "emission_plan": {"bindings": [{"hook": "onChargeTick", "transitions": [0, 1, 2]}], "event_bindings": []},
            }},
        })).unwrap();
        let bundle = compile_bundle(
            &BTreeMap::new(),
            &BTreeMap::from([("mario".to_string(), charts)]),
        )
        .unwrap()
        .unwrap();
        let coordinator = controller_protocol::decode_request(&bundle.encoded)
            .unwrap()
            .coordinators()
            .unwrap()
            .remove("mario")
            .unwrap();
        let inputs = coordinator.native_inputs();
        let mut state = coordinator.initial_state();
        let mut step = |live: bool, guard: bool| {
            let observations: Vec<_> = inputs
                .iter()
                .map(|input| match input {
                    NativeInput::OwnedObject { .. } => NativeObservation::Flag(live),
                    _ => NativeObservation::Float(-1.0),
                })
                .collect();
            let mut snapshot = NativeSnapshot::from_observations(&inputs, &observations).unwrap();
            let buttons = [
                InputButton::Attack,
                InputButton::Special,
                InputButton::Jump,
                InputButton::Guard,
                InputButton::Grab,
            ]
            .into_iter()
            .flat_map(|b| {
                [ButtonEdge::Held, ButtonEdge::Pressed, ButtonEdge::Released].map(|e| {
                    (
                        (b, e),
                        guard && b == InputButton::Guard && e == ButtonEdge::Pressed,
                    )
                })
            })
            .collect();
            snapshot.controller = Some(ControllerInput {
                facing: -1.0,
                buttons,
                ..Default::default()
            });
            let result = coordinator.step_frame(&mut state, &snapshot).unwrap();
            let fired = result
                .charts
                .iter()
                .find(|c| c.chart_id == "steer_projectile")
                .unwrap()
                .result
                .fired
                .iter()
                .map(|f| f.index)
                .collect::<Vec<_>>();
            let ops = result
                .engine_actions
                .into_iter()
                .filter_map(|tagged| match tagged.action {
                    Action::Object {
                        op:
                            ObjectOp::SetSpeed {
                                x: Expr::Const(Value::Float(x)),
                                ..
                            },
                        ..
                    } => Some(format!("speed {x}")),
                    Action::Object {
                        op: ObjectOp::Remove,
                        ..
                    } => Some("remove".into()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            (fired, ops)
        };
        // Thrown facing left; a guard press while it flies runs the third rule after the first.
        assert_eq!(step(false, false), (vec![1], vec![]));
        assert_eq!(step(true, false), (vec![0], vec!["speed -1".to_string()]));
        assert_eq!(
            step(true, true),
            (vec![0, 2], vec!["speed -1".to_string(), "remove".into()])
        );
        // The press is one edge: holding the trigger bit does not remove again.
        assert_eq!(step(true, true), (vec![0], vec!["speed -1".to_string()]));
        // Once the projectile is gone (consumed by a hit), a press has nothing to remove.
        assert_eq!(step(false, true), (vec![1], vec![]));
    }
}
