use std::collections::BTreeMap;

use super::native_inputs::{
    ButtonEdge, CombatChannel, CombatOp, ControllerInput, FighterValue, InputButton, MovementStat,
    NativeInput, NativeKind, NativeSnapshot, PolicyScope,
};
use super::{adapter, behavior::*, codegen, hooks, live_model as model, live_protocol};

fn always() -> Predicate {
    Predicate::Compare {
        op: CmpOp::Eq,
        left: Expr::Const(ConstValue::Int(1)),
        right: Expr::Const(ConstValue::Int(1)),
    }
}
fn fixture() -> BehaviorGraph {
    BehaviorGraph {
        custom_statuses: Default::default(),
        initial: "active".into(),
        states: vec![StateDef {
            status: None,
            name: "active".into(),
        }],
        vars: vec![
            VarDef {
                keep_across_form_switch: false,
                name: "x".into(),
                kind: VarType::Float,
                initial: ConstValue::Float(0.0),
            },
            VarDef {
                keep_across_form_switch: false,
                name: "n".into(),
                kind: VarType::Int,
                initial: ConstValue::Int(0),
            },
        ],
        transitions: vec![Transition {
            from: "active".into(),
            to: "active".into(),
            guard: always(),
            on_frame: true,
            actions: vec![],
        }],
        emission_plan: codegen::EmissionPlan {
            bindings: vec![codegen::HookBinding {
                hook: hooks::HookName::OnChargeTick,
                article: None,
                transitions: vec![0],
            }],
            ..Default::default()
        },
    }
}
fn program(graph: &BehaviorGraph) -> model::LiveProgram {
    live_protocol::program_from_graph("robot", graph).unwrap()
}
fn input_snapshot(button: InputButton, edge: ButtonEdge, active: bool) -> NativeSnapshot {
    let mut snapshot = NativeSnapshot::empty();
    snapshot.controller = Some(ControllerInput {
        buttons: BTreeMap::from([((button, edge), active)]),
        stick_x: 0.75,
        stick_y: -0.25,
        facing: -1.0,
    });
    snapshot
}

#[test]
fn controller_buttons_and_axes_require_frame_scope_and_fresh_input() {
    for button in [
        InputButton::Attack,
        InputButton::Special,
        InputButton::Jump,
        InputButton::Guard,
        InputButton::Grab,
    ] {
        for edge in [ButtonEdge::Held, ButtonEdge::Pressed, ButtonEdge::Released] {
            let mut graph = fixture();
            graph.transitions[0].guard = Predicate::Button { button, edge };
            graph.transitions[0].actions = vec![Action::Set {
                var: "x".into(),
                value: Expr::Add(
                    Box::new(Expr::StickX {
                        relative_to_facing: true,
                    }),
                    Box::new(Expr::StickY),
                ),
            }];
            assert!(validate_for_fighter("robot", &graph).is_empty());
            let good = program(&graph);
            let mut state = good.initial_state();
            assert!(good
                .step_frame_with_snapshot(&mut state, &NativeSnapshot::empty())
                .is_err());
            let no = good
                .step_frame_with_snapshot(&mut state, &input_snapshot(button, edge, false))
                .unwrap();
            assert!(no.fired.is_empty());
            let yes = good
                .step_frame_with_snapshot(&mut state, &input_snapshot(button, edge, true))
                .unwrap();
            assert_eq!(yes.after.values["x"], model::Value::Float(-1.0));
            for hook in [hooks::HookName::OnStockStart, hooks::HookName::OnTimeout] {
                let mut bad = graph.clone();
                bad.emission_plan.bindings[0].hook = hook;
                assert!(validate(&bad)
                    .iter()
                    .any(|e| e.reason.contains("frame-tick")));
            }
            let mut bad = good.clone();
            bad.bindings[0].hook = model::HookName::OnStockStart;
            assert!(bad
                .validate()
                .unwrap_err()
                .iter()
                .any(|e| e.reason.contains("frame-tick")));
            graph
                .emission_plan
                .event_bindings
                .push(codegen::EventBinding {
                    name: "event".into(),
                    transitions: vec![0],
                    fields: vec![],
                });
            assert!(!validate(&graph).is_empty());
        }
    }
}

#[test]
fn controller_input_scope_checks_action_and_event_value_operands() {
    let mut graph = fixture();
    graph.transitions[0].actions = vec![Action::Set {
        var: "x".into(),
        value: Expr::StickX {
            relative_to_facing: false,
        },
    }];
    assert!(validate(&graph).is_empty());
    graph.emission_plan.bindings.clear();
    assert!(validate(&graph)
        .iter()
        .any(|e| e.reason.contains("frame-tick")));
    let mut graph = fixture();
    graph.transitions[0].actions = vec![Action::Set {
        var: "x".into(),
        value: Expr::StickY,
    }];
    graph.emission_plan.bindings[0].hook = hooks::HookName::OnStockStart;
    assert!(live_protocol::program_from_graph("robot", &graph).is_err());
}

#[test]
fn bounded_rng_is_repeatable_per_chart_and_rejects_bad_ranges_and_types() {
    let mut graph = fixture();
    graph.transitions[0].actions = vec![
        Action::Set {
            var: "n".into(),
            value: Expr::RandomInt { min: -10, max: 10 },
        },
        Action::Set {
            var: "x".into(),
            value: Expr::RandomFloat {
                min: -0.5,
                max: 0.75,
            },
        },
    ];
    let good = program(&graph);
    let mut a = good.initial_state();
    let mut b = good.initial_state();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..100 {
        let first = good.step_frame(&mut a).unwrap();
        let second = good.step_frame(&mut b).unwrap();
        assert_eq!(first.after, second.after);
        assert_eq!(a.random_seed, b.random_seed);
        let model::Value::Int(n) = a.values["n"] else {
            panic!()
        };
        let model::Value::Float(x) = a.values["x"] else {
            panic!()
        };
        assert!((-10..=10).contains(&n));
        assert!((-0.5..=0.75).contains(&x));
        seen.insert(n);
    }
    assert!(seen.len() > 10);
    for (min, max) in [(2, 1), (i32::MIN, i32::MAX), (0, 1_000_001)] {
        let mut bad = graph.clone();
        bad.transitions[0].actions[0] = Action::Set {
            var: "n".into(),
            value: Expr::RandomInt { min, max },
        };
        assert!(validate(&bad)
            .iter()
            .any(|e| e.reason.contains("random integer")));
        let mut wire = good.clone();
        wire.transitions[0].actions[0] = model::Action::Set {
            var: "n".into(),
            value: model::Expr::RandomInt { min, max },
        };
        assert!(wire.validate().is_err());
    }
    for (min, max) in [(1.0, 0.0), (f32::NAN, 1.0), (0.0, f32::INFINITY)] {
        let mut bad = graph.clone();
        bad.transitions[0].actions[1] = Action::Set {
            var: "x".into(),
            value: Expr::RandomFloat { min, max },
        };
        assert!(validate(&bad)
            .iter()
            .any(|e| e.reason.contains("random float")));
    }
    graph.transitions[0].actions[0] = Action::Set {
        var: "n".into(),
        value: Expr::RandomInt { min: 7, max: 7 },
    };
    graph.transitions[0].actions[1] = Action::Set {
        var: "x".into(),
        value: Expr::RandomFloat { min: 0.5, max: 0.5 },
    };
    let fixed = program(&graph);
    let result = fixed.step_frame(&mut fixed.initial_state()).unwrap();
    assert_eq!(result.after.values["n"], model::Value::Int(7));
    assert_eq!(result.after.values["x"], model::Value::Float(0.5));
    graph.transitions[0].actions[0] = Action::Set {
        var: "n".into(),
        value: Expr::RandomInt {
            min: 0,
            max: 1_000_000,
        },
    };
    assert!(validate(&graph).is_empty());
    graph.transitions[0].actions[0] = Action::Set {
        var: "x".into(),
        value: Expr::RandomInt { min: 0, max: 1 },
    };
    assert!(!validate(&graph).is_empty());
}

fn work(symbol: &str, kind: NativeKind) -> NativeInput {
    NativeInput::from_registry("robot", symbol, kind).unwrap()
}
#[test]
fn native_actions_resolve_values_at_the_action_and_validate_exact_owner_kind_and_helper() {
    let float = work(
        "FIGHTER_ROBOT_INSTANCE_WORK_ID_FLOAT_BEAM_ENERGY_VALUE",
        NativeKind::Float,
    );
    let int = work(
        "FIGHTER_ROBOT_INSTANCE_WORK_ID_INT_BEAM_GLOW_ATTACK",
        NativeKind::Int,
    );
    let flag = work(
        "FIGHTER_ROBOT_INSTANCE_WORK_ID_FLAG_ARMSPIN_PUSH_B",
        NativeKind::Flag,
    );
    let mut graph = fixture();
    graph.transitions[0].actions = vec![
        Action::Set {
            var: "x".into(),
            value: Expr::Const(ConstValue::Float(3.0)),
        },
        Action::SetNative {
            input: float.clone(),
            value: Expr::Var("x".into()),
        },
        Action::AddNative {
            input: int.clone(),
            value: Expr::RandomInt { min: 2, max: 2 },
        },
        Action::SetNativeFlag {
            input: flag,
            value: always(),
        },
    ];
    assert!(validate_for_fighter("robot", &graph).is_empty());
    let good = program(&graph);
    let result = good.step_frame(&mut good.initial_state()).unwrap();
    assert_eq!(result.engine_actions.len(), 3);
    assert_eq!(
        result.engine_actions[0],
        model::Action::SetNative {
            input: float.clone(),
            value: model::Expr::Const(model::Value::Float(3.0))
        }
    );
    assert_eq!(
        result.engine_actions[1],
        model::Action::AddNative {
            input: int.clone(),
            value: model::Expr::Const(model::Value::Int(2))
        }
    );
    assert!(!validate_for_fighter("mario", &graph).is_empty());
    for input in [
        int,
        NativeInput::Fighter {
            value: FighterValue::Damage,
            fighter: "robot".into(),
        },
        NativeInput::Unsupported("invented".into()),
    ] {
        let mut bad = graph.clone();
        bad.transitions[0].actions[1] = Action::SetNative {
            input,
            value: Expr::Const(ConstValue::Float(1.0)),
        };
        assert!(!validate_for_fighter("robot", &bad).is_empty());
        assert!(live_protocol::program_from_graph("robot", &bad).is_err());
    }
    for input in [
        NativeInput::CloudLimitGauge,
        NativeInput::from_registry(
            "cloud",
            "FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE",
            NativeKind::Float,
        )
        .unwrap(),
    ] {
        let mut bad = graph.clone();
        bad.transitions[0].actions = vec![Action::SetNative {
            input,
            value: Expr::Const(ConstValue::Float(1.0)),
        }];
        assert!(validate_for_fighter("cloud", &bad)
            .iter()
            .any(|e| e.reason.contains("helper")));
        let mut wire = good.clone();
        wire.fighter = "cloud".into();
        wire.transitions[0].actions = vec![model::Action::SetNative {
            input: NativeInput::CloudLimitGauge,
            value: model::Expr::Const(model::Value::Float(1.0)),
        }];
        assert!(wire.validate().is_err());
    }
}

#[test]
fn controller_primitives_serde_and_static_bundle_keep_live_program_parity() {
    let mut graph = fixture();
    graph.transitions[0].guard = Predicate::Button {
        button: InputButton::Special,
        edge: ButtonEdge::Pressed,
    };
    graph.transitions[0].actions = vec![
        Action::Set {
            var: "x".into(),
            value: Expr::StickX {
                relative_to_facing: true,
            },
        },
        Action::SetNative {
            input: work(
                "FIGHTER_ROBOT_INSTANCE_WORK_ID_INT_BEAM_GLOW_ATTACK",
                NativeKind::Int,
            ),
            value: Expr::RandomInt { min: 1, max: 6 },
        },
    ];
    let json = serde_json::to_value(&graph).unwrap();
    assert_eq!(
        json["transitions"][0]["guard"],
        serde_json::json!({"Button":{"button":"special","edge":"Pressed"}})
    );
    assert_eq!(
        json["transitions"][0]["actions"][0]["Set"]["value"],
        serde_json::json!({"StickX":{"relative_to_facing":true}})
    );
    let decoded: BehaviorGraph = serde_json::from_value(json).unwrap();
    assert_eq!(decoded, graph);
    assert_eq!(
        serde_json::from_str::<Expr>(r#"{"StickX":{}}"#).unwrap(),
        Expr::StickX {
            relative_to_facing: false
        }
    );
    assert!(
        serde_json::from_str::<Predicate>(r#"{"Button":{"button":"invented","edge":"Held"}}"#)
            .is_err()
    );
    let rendered = adapter::render(&BTreeMap::from([("robot".into(), graph.clone())])).unwrap();
    let line = rendered
        .source
        .lines()
        .find(|line| line.starts_with("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = "))
        .unwrap();
    let encoded: String = serde_json::from_str(
        line.strip_prefix("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = ")
            .unwrap()
            .strip_suffix(';')
            .unwrap(),
    )
    .unwrap();
    let bundle = super::controller_protocol::decode_request(&encoded).unwrap();
    assert_eq!(bundle.charts.len(), 1);
    assert_eq!(bundle.charts[0].program, program(&graph));
    let plugin = include_str!("../../plugins/slight_replica/src/slight/gimmicks/native_values.rs");
    for call in [
        "check_button_on",
        "check_button_trigger",
        "check_button_release",
        "get_stick_x",
        "get_stick_y",
        "PostureModule::lr",
        "WorkModule::set_float",
        "WorkModule::set_int",
        "WorkModule::on_flag",
        "WorkModule::off_flag",
        "input.validate_write(fighter)?",
        "require_identity(boid, fighter, expected)?",
    ] {
        assert!(plugin.contains(call), "missing {call}");
    }
    assert!(
        include_str!("../../plugins/slight_replica/src/slight/gimmicks/mod.rs")
            .contains("smash::app::sv_math::rand(smash::hash40(\"fighter\"), upper)")
    );
}

#[test]
fn owned_article_count_actions_and_filters_survive_static_and_live_bundles() {
    const ARTICLE: &str = "FIGHTER_ROBOT_GENERATE_ARTICLE_GYRO";
    const STATUS: &str = "WEAPON_ROBOT_GYRO_STATUS_KIND_HOLD";
    let mut graph = fixture();
    graph.transitions[0].actions = vec![
        Action::Set {
            var: "n".into(),
            value: Expr::NativeInt(NativeInput::article_count(ARTICLE)),
        },
        Action::RequestArticleStatus {
            symbol: ARTICLE.into(),
            status: STATUS.into(),
        },
        Action::RemoveNewestArticle {
            symbol: ARTICLE.into(),
        },
    ];
    assert!(
        validate_for_fighter("robot", &graph).is_empty(),
        "{:?}",
        validate_for_fighter("robot", &graph)
    );
    let good = program(&graph);
    let source = NativeInput::article_count(ARTICLE);
    let snapshot = NativeSnapshot::from_observations(
        &[source],
        &[super::native_inputs::NativeObservation::Int(3)],
    )
    .unwrap();
    let mut state = good.initial_state();
    let result = good
        .step_frame_with_snapshot(&mut state, &snapshot)
        .unwrap();
    assert_eq!(state.values["n"], model::Value::Int(3));
    assert_eq!(result.engine_actions.len(), 2);
    assert!(good.step_frame(&mut good.initial_state()).is_err());
    let rendered = adapter::render(&BTreeMap::from([("robot".into(), graph.clone())])).unwrap();
    let line = rendered
        .source
        .lines()
        .find(|line| line.starts_with("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = "))
        .unwrap();
    let encoded: String = serde_json::from_str(
        line.strip_prefix("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = ")
            .unwrap()
            .strip_suffix(';')
            .unwrap(),
    )
    .unwrap();
    let bundle = super::controller_protocol::decode_request(&encoded).unwrap();
    assert_eq!(bundle.charts[0].program.transitions, good.transitions);
    assert_eq!(
        serde_json::to_value(&graph.transitions[0].actions[0]).unwrap()["Set"]["value"],
        serde_json::json!({"NativeInt":format!("article_count:robot:{ARTICLE}")})
    );
    // Projects saved before the article count joined NativeInt still load.
    let legacy: Expr =
        serde_json::from_value(serde_json::json!({"ArticleCount":{"symbol":ARTICLE}})).unwrap();
    assert_eq!(legacy, Expr::NativeInt(NativeInput::article_count(ARTICLE)));
    for bad in [
        "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE",
        "FIGHTER_ROBOT_INSTANCE_WORK_ID_INT_BEAM_ENERGY",
        "missing",
    ] {
        let mut wrong = graph.clone();
        wrong.transitions[0].actions[0] = Action::Set {
            var: "n".into(),
            value: Expr::NativeInt(NativeInput::article_count(bad)),
        };
        assert!(
            !validate_for_fighter("robot", &wrong).is_empty(),
            "accepted {bad}"
        );
        assert!(live_protocol::program_from_graph("robot", &wrong).is_err());
    }
    for action in [
        Action::RemoveNewestArticle {
            symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
        },
        Action::RequestArticleStatus {
            symbol: ARTICLE.into(),
            status: "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR".into(),
        },
    ] {
        let mut wrong = graph.clone();
        wrong.transitions[0].actions = vec![action];
        assert!(!validate_for_fighter("robot", &wrong).is_empty());
    }
    graph.transitions[0].actions = vec![Action::Add {
        var: "n".into(),
        value: Expr::Const(ConstValue::Int(1)),
    }];
    for hook in [
        hooks::HookName::OnArticleSpawn,
        hooks::HookName::OnArticleRemoved,
    ] {
        graph.emission_plan.bindings[0].hook = hook;
        graph.emission_plan.bindings[0].article = Some(ARTICLE.into());
        let good = program(&graph);
        let hook = if hook == hooks::HookName::OnArticleSpawn {
            model::HookName::OnArticleSpawn
        } else {
            model::HookName::OnArticleRemoved
        };
        let mut state = good.initial_state();
        let mut payload = NativeSnapshot::empty();
        payload.article_event = Some((hook == model::HookName::OnArticleSpawn, ARTICLE.into()));
        good.step_with_snapshot(&mut state, hook, &payload).unwrap();
        assert_eq!(state.values["n"], model::Value::Int(1));
        for event in [
            None,
            Some((hook != model::HookName::OnArticleSpawn, ARTICLE.into())),
            Some((
                hook == model::HookName::OnArticleSpawn,
                "FIGHTER_ROBOT_GENERATE_ARTICLE_BEAM".into(),
            )),
        ] {
            let mut payload = NativeSnapshot::empty();
            payload.article_event = event;
            good.step_with_snapshot(&mut state, hook, &payload).unwrap();
            assert_eq!(state.values["n"], model::Value::Int(1));
        }
        let mut wrong = good.clone();
        wrong.bindings[0].article = Some("FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into());
        assert!(wrong.validate().is_err());
    }
}

#[test]
fn phase_reads_require_exact_status_fresh_frame_and_exclusive_tick_scope() {
    use super::native_inputs::{PhaseInput, SituationKind};
    const STATUS: &str = "FIGHTER_STATUS_KIND_SPECIAL_N";
    let mut graph = fixture();
    graph.transitions[0].guard = Predicate::And(
        Box::new(Predicate::InStatus {
            symbol: STATUS.into(),
        }),
        Box::new(Predicate::Situation {
            kind: SituationKind::Ground,
        }),
    );
    graph.transitions[0].actions = vec![
        Action::Set {
            var: "n".into(),
            value: Expr::StatusFrame,
        },
        Action::Set {
            var: "x".into(),
            value: Expr::MotionFrame,
        },
    ];
    let good = program(&graph);
    assert!(good.uses_controller_input());
    assert!(good.uses_phase_input());
    let rendered = adapter::render(&BTreeMap::from([("robot".into(), graph.clone())])).unwrap();
    let line = rendered
        .source
        .lines()
        .find(|s| s.starts_with("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = "))
        .unwrap();
    let encoded: String = serde_json::from_str(
        line.strip_prefix("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = ")
            .unwrap()
            .strip_suffix(';')
            .unwrap(),
    )
    .unwrap();
    let bundle = super::controller_protocol::decode_request(&encoded).unwrap();
    assert_eq!(bundle.charts[0].program, good);
    assert!(good
        .required_native_features()
        .contains(super::native_inputs::STATUS_PHASE_FEATURE));
    let mut snapshot = NativeSnapshot::empty();
    snapshot.phase = Some(PhaseInput {
        fighter: "robot".into(),
        status: super::registry::resolve(
            "robot",
            STATUS,
            super::registry::IdentifierKind::FighterStatus,
        )
        .unwrap()
        .value as i32,
        status_frame: 17,
        motion_frame: 8.5,
        motion: 0,
        situation: Some(SituationKind::Ground),
    });
    let mut state = good.initial_state();
    assert!(good.step_frame(&mut state).is_err());
    let result = good
        .step_frame_with_snapshot(&mut state, &snapshot)
        .unwrap();
    assert_eq!(result.after.values["n"], model::Value::Int(17));
    assert_eq!(result.after.values["x"], model::Value::Float(8.5));
    for (status, situation) in [
        (0, Some(SituationKind::Ground)),
        (
            snapshot.phase.as_ref().unwrap().status,
            Some(SituationKind::Air),
        ),
        (snapshot.phase.as_ref().unwrap().status, None),
    ] {
        let mut wrong = snapshot.clone();
        wrong.phase.as_mut().unwrap().status = status;
        wrong.phase.as_mut().unwrap().situation = situation;
        assert!(good
            .step_frame_with_snapshot(&mut good.initial_state(), &wrong)
            .unwrap()
            .fired
            .is_empty());
    }
    let mut wrong = snapshot.clone();
    wrong.phase.as_mut().unwrap().fighter = "mario".into();
    assert!(good.step_frame_with_snapshot(&mut state, &wrong).is_err());
    wrong = snapshot.clone();
    wrong.phase.as_mut().unwrap().motion_frame = f32::NAN;
    assert!(good.step_frame_with_snapshot(&mut state, &wrong).is_err());
    for symbol in [
        "FIGHTER_CLOUD_STATUS_KIND_SPECIAL_LW_CHARGE",
        "WEAPON_ROBOT_BEAM_STATUS_KIND_FLY",
        "FIGHTER_STATUS_KIND_INVENTED",
    ] {
        let mut bad = graph.clone();
        bad.transitions[0].guard = Predicate::InStatus {
            symbol: symbol.into(),
        };
        assert!(!validate_for_fighter("robot", &bad).is_empty(), "{symbol}");
        let mut wire = good.clone();
        wire.transitions[0].guard = model::Predicate::InStatus {
            symbol: symbol.into(),
        };
        assert!(wire.validate().is_err(), "{symbol}");
    }
    for hook in [
        hooks::HookName::OnStockStart,
        hooks::HookName::OnStatusEnter,
        hooks::HookName::OnTakeDamage,
        hooks::HookName::OnTimeout,
    ] {
        let mut bad = graph.clone();
        bad.emission_plan.bindings[0].hook = hook;
        assert!(!validate(&bad).is_empty());
        assert!(live_protocol::program_from_graph("robot", &bad).is_err());
        let mut wire = good.clone();
        wire.bindings[0].hook = model::HookName::OnStockStart;
        assert!(wire.validate().is_err());
    }
    let mut bad = graph.clone();
    bad.transitions[0].on_frame = false;
    assert!(!validate(&bad).is_empty());
    let json = serde_json::to_value(&graph).unwrap();
    assert_eq!(
        json["transitions"][0]["actions"][0]["Set"]["value"],
        "StatusFrame"
    );
    assert_eq!(
        json["transitions"][0]["actions"][1]["Set"]["value"],
        "MotionFrame"
    );
    assert_eq!(
        serde_json::to_value(&graph.transitions[0].guard).unwrap(),
        serde_json::json!({"And":[{"InStatus":{"symbol":STATUS}},{"Situation":{"kind":"ground"}}]})
    );
}

#[test]
fn phase_actions_resolve_at_action_point_reject_bounds_and_export_identically() {
    let mut graph = fixture();
    graph.transitions[0].actions = vec![
        Action::Set {
            var: "x".into(),
            value: Expr::Const(ConstValue::Float(1.5)),
        },
        Action::SetMotionRate {
            rate: Expr::Var("x".into()),
        },
        Action::AddSpeed {
            x: Expr::Var("x".into()),
            y: Expr::Const(ConstValue::Float(-0.5)),
        },
    ];
    let good = program(&graph);
    assert!(!good.uses_phase_input());
    let result = good.step_frame(&mut good.initial_state()).unwrap();
    assert_eq!(
        result.engine_actions,
        vec![
            model::Action::SetMotionRate {
                rate: model::Expr::Const(model::Value::Float(1.5))
            },
            model::Action::AddSpeed {
                x: model::Expr::Const(model::Value::Float(1.5)),
                y: model::Expr::Const(model::Value::Float(-0.5))
            },
        ]
    );
    for action in &result.engine_actions {
        assert!(super::phase_commands::resolve(action).is_ok());
    }
    for (rate, speed) in [(0.05, -10.0), (20.0, 10.0)] {
        let mut valid = graph.clone();
        valid.transitions[0].actions = vec![
            Action::SetMotionRate {
                rate: Expr::Const(ConstValue::Float(rate)),
            },
            Action::AddSpeed {
                x: Expr::Const(ConstValue::Float(speed)),
                y: Expr::Const(ConstValue::Float(speed)),
            },
        ];
        assert!(validate(&valid).is_empty());
        assert!(program(&valid)
            .step_frame(&mut program(&valid).initial_state())
            .is_ok());
    }
    for action in [
        Action::SetMotionRate {
            rate: Expr::Const(ConstValue::Float(0.0)),
        },
        Action::SetMotionRate {
            rate: Expr::Const(ConstValue::Float(20.01)),
        },
        Action::AddSpeed {
            x: Expr::Const(ConstValue::Float(10.01)),
            y: Expr::Const(ConstValue::Float(0.0)),
        },
        Action::AddSpeed {
            x: Expr::Const(ConstValue::Float(0.0)),
            y: Expr::Const(ConstValue::Int(1)),
        },
    ] {
        let mut bad = graph.clone();
        bad.transitions[0].actions = vec![action];
        assert!(!validate(&bad).is_empty());
        assert!(live_protocol::program_from_graph("robot", &bad).is_err());
    }
    // Dynamic expressions may validate structurally, but cannot bypass bounds at dispatch.
    for value in [21.0, -11.0] {
        let mut bad = good.clone();
        bad.transitions[0].actions[0] = model::Action::Set {
            var: "x".into(),
            value: model::Expr::Const(model::Value::Float(value)),
        };
        assert!(bad.validate().is_ok());
        let mut state = bad.initial_state();
        let before = state.clone();
        assert!(bad.step_frame(&mut state).is_err());
        assert_eq!(state, before);
    }
    assert!(
        super::phase_commands::resolve(&model::Action::SetMotionRate {
            rate: model::Expr::Var("x".into())
        })
        .is_err()
    );
    assert!(super::phase_commands::resolve(&model::Action::AddSpeed {
        x: model::Expr::Const(model::Value::Float(0.0)),
        y: model::Expr::Const(model::Value::Float(f32::NAN))
    })
    .is_err());
    let rendered = adapter::render(&BTreeMap::from([("robot".into(), graph.clone())])).unwrap();
    let line = rendered
        .source
        .lines()
        .find(|s| s.starts_with("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = "))
        .unwrap();
    let encoded: String = serde_json::from_str(
        line.strip_prefix("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = ")
            .unwrap()
            .strip_suffix(';')
            .unwrap(),
    )
    .unwrap();
    let bundle = super::controller_protocol::decode_request(&encoded).unwrap();
    assert_eq!(bundle.charts[0].program, good);
    let json = serde_json::to_value(&graph).unwrap();
    assert_eq!(
        json["transitions"][0]["actions"][1],
        serde_json::json!({"SetMotionRate":{"rate":{"Var":"x"}}})
    );
    assert_eq!(
        json["transitions"][0]["actions"][2],
        serde_json::json!({"AddSpeed":{"x":{"Var":"x"},"y":{"Const":{"Float":-0.5}}}})
    );
    assert_eq!(
        serde_json::from_value::<BehaviorGraph>(json).unwrap(),
        graph
    );
}

#[test]
fn each_phase_source_independently_enforces_hook_scope() {
    use super::native_inputs::SituationKind;
    let cases = [
        (
            always(),
            vec![Action::Set {
                var: "n".into(),
                value: Expr::StatusFrame,
            }],
        ),
        (
            always(),
            vec![Action::Set {
                var: "x".into(),
                value: Expr::MotionFrame,
            }],
        ),
        (
            Predicate::InStatus {
                symbol: "FIGHTER_STATUS_KIND_WAIT".into(),
            },
            vec![],
        ),
        (
            Predicate::Situation {
                kind: SituationKind::Air,
            },
            vec![],
        ),
    ];
    for (guard, actions) in cases {
        let mut graph = fixture();
        graph.transitions[0].guard = guard;
        graph.transitions[0].actions = actions;
        assert!(validate_for_fighter("robot", &graph).is_empty());
        let good = program(&graph);
        for hook in [hooks::HookName::OnStockStart, hooks::HookName::OnTimeout] {
            let mut bad = graph.clone();
            bad.emission_plan.bindings[0].hook = hook;
            assert!(validate(&bad)
                .iter()
                .any(|e| e.reason.contains("frame-tick")));
            let mut wire = good.clone();
            wire.bindings[0].hook = model::HookName::OnStockStart;
            assert!(wire.validate().is_err());
        }
        let mut bad = graph.clone();
        bad.transitions[0].on_frame = false;
        assert!(!validate(&bad).is_empty());
        let mut wire = good.clone();
        wire.transitions[0].on_frame = false;
        assert!(wire.validate().is_err());
        bad = graph.clone();
        bad.emission_plan
            .event_bindings
            .push(codegen::EventBinding {
                name: "named".into(),
                fields: vec![],
                transitions: vec![0],
            });
        assert!(!validate(&bad).is_empty());
    }
    // The speed expression has its own runtime bound, independently of rate.
    let mut graph = fixture();
    graph.transitions[0].actions = vec![Action::AddSpeed {
        x: Expr::Var("x".into()),
        y: Expr::Var("x".into()),
    }];
    let good = program(&graph);
    let mut state = good.initial_state();
    state.values.insert("x".into(), model::Value::Float(10.0));
    assert!(good.step_frame(&mut state).is_ok());
    for value in [-10.01, 10.01, f32::NAN] {
        state.values.insert("x".into(), model::Value::Float(value));
        assert!(good.step_frame(&mut state).is_err());
    }
}

#[test]
fn damage_modifier_actions_resolve_formulas_and_roundtrip_static_and_wire() {
    use super::controller::ControllerAction;
    let mut graph = fixture();
    let float = |v| Expr::Const(ConstValue::Float(v));
    graph.transitions[0].actions = vec![
        Action::Set {
            var: "x".into(),
            value: float(0.75),
        },
        Action::AddDamage {
            amount: Expr::Mul(Box::new(Expr::Var("x".into())), Box::new(float(-10.0))),
        },
        policy(CombatOp::DamageTaken(Expr::Var("x".into()))),
        policy(CombatOp::KnockbackTaken(float(1.25))),
        policy(CombatOp::Armor(float(12.0))),
        policy(CombatOp::SuperArmor),
        policy(CombatOp::Clear(CombatChannel::Armor)),
        policy(CombatOp::DamageTaken(float(1.0))),
        policy(CombatOp::KnockbackTaken(float(1.0))),
        Action::CombatPolicy {
            scope: PolicyScope::Frames(30),
            op: CombatOp::Intangible,
        },
        Action::CombatPolicy {
            scope: PolicyScope::UntilCleared,
            op: CombatOp::DamageDealt(float(1.5)),
        },
        Action::CombatPolicy {
            scope: PolicyScope::Frames(120),
            op: CombatOp::KnockbackDealt(float(2.0)),
        },
        Action::CombatPolicy {
            scope: PolicyScope::UntilCleared,
            op: CombatOp::Stat(MovementStat::RunSpeed, float(1.5)),
        },
    ];
    for action in &graph.transitions[0].actions[1..] {
        let typed = ControllerAction::try_from(action).unwrap();
        assert_eq!(typed.to_ir("robot").unwrap(), *action);
    }
    let live = program(&graph);
    let result = live.step_frame(&mut live.initial_state()).unwrap();
    use super::phase_commands::{resolve, Command};
    assert_eq!(
        result
            .engine_actions
            .iter()
            .map(resolve)
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        vec![
            Command::AddDamage(-7.5),
            status(CombatOp::DamageTaken(0.75)),
            status(CombatOp::KnockbackTaken(1.25)),
            status(CombatOp::Armor(12.0)),
            status(CombatOp::SuperArmor),
            status(CombatOp::Clear(CombatChannel::Armor)),
            status(CombatOp::DamageTaken(1.0)),
            status(CombatOp::KnockbackTaken(1.0)),
            Command::Combat(PolicyScope::Frames(30), CombatOp::Intangible),
            Command::Combat(PolicyScope::UntilCleared, CombatOp::DamageDealt(1.5)),
            Command::Combat(PolicyScope::Frames(120), CombatOp::KnockbackDealt(2.0)),
            Command::Combat(
                PolicyScope::UntilCleared,
                CombatOp::Stat(MovementStat::RunSpeed, 1.5)
            ),
        ]
    );
    let json = serde_json::to_string(&graph).unwrap();
    assert_eq!(serde_json::from_str::<BehaviorGraph>(&json).unwrap(), graph);
    let wire = serde_json::to_string(&live).unwrap();
    let decoded: model::LiveProgram = serde_json::from_str(&wire).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded, live);
    let rendered = adapter::render(&BTreeMap::from([("robot".into(), graph)])).unwrap();
    let line = rendered
        .source
        .lines()
        .find(|s| s.starts_with("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = "))
        .unwrap();
    let encoded: String = serde_json::from_str(
        line.strip_prefix("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = ")
            .unwrap()
            .strip_suffix(';')
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        super::controller_protocol::decode_request(&encoded)
            .unwrap()
            .charts[0]
            .program,
        live
    );
}

#[test]
fn damage_modifiers_reject_wrong_types_constants_and_dynamic_overflow_atomically() {
    let float = |v| Expr::Const(ConstValue::Float(v));
    let actions: [fn(Expr) -> Action; 6] = [
        |amount| Action::AddDamage { amount },
        |v| policy(CombatOp::DamageTaken(v)),
        |v| policy(CombatOp::KnockbackTaken(v)),
        |v| policy(CombatOp::Armor(v)),
        |v| policy(CombatOp::KnockbackDealt(v)),
        |v| policy(CombatOp::Stat(MovementStat::Gravity, v)),
    ];
    for (make, valid, invalid) in [
        (actions[0], vec![-999.0, 0.0, 999.0], vec![-999.01, 999.01]),
        (actions[1], vec![0.0, 1.0, 10.0], vec![-0.01, 10.01]),
        (actions[2], vec![0.0, 1.0, 10.0], vec![-0.01, 10.01]),
        (actions[3], vec![0.0, 999.0], vec![-0.01, 999.01]),
        (actions[4], vec![0.0, 1.0, 10.0], vec![-0.01, 10.01]),
        (actions[5], vec![0.1, 1.0, 10.0], vec![0.0, 0.09, 10.01]),
    ] {
        let mut graph = fixture();
        for value in valid {
            graph.transitions[0].actions = vec![make(float(value))];
            let live = program(&graph);
            live.step_frame(&mut live.initial_state()).unwrap();
        }
        for value in invalid
            .into_iter()
            .chain([f32::NAN, f32::INFINITY, f32::NEG_INFINITY])
        {
            graph.transitions[0].actions = vec![make(float(value))];
            assert!(!validate(&graph).is_empty());
            assert!(live_protocol::program_from_graph("robot", &graph).is_err());
            graph.transitions[0].actions = vec![
                Action::Set {
                    var: "n".into(),
                    value: Expr::Const(ConstValue::Int(7)),
                },
                make(Expr::Var("x".into())),
            ];
            let mut live = program(&graph);
            let mut state = live.initial_state();
            state.values.insert("x".into(), model::Value::Float(value));
            let before = state.clone();
            assert!(live.step_frame(&mut state).is_err());
            assert_eq!(state.state, before.state);
            assert_eq!(state.random_seed, before.random_seed);
            assert_eq!(state.values["n"], before.values["n"]);
            let model::Value::Float(after_x) = state.values["x"] else {
                panic!("changed type")
            };
            assert_eq!(after_x.to_bits(), value.to_bits());
            // The wire-side validator must independently reject bad literals.
            live.transitions[0].actions[1] = match &live.transitions[0].actions[1] {
                model::Action::AddDamage { .. } => model::Action::AddDamage {
                    amount: model::Expr::Const(model::Value::Float(value)),
                },
                model::Action::CombatPolicy { scope, op } => model::Action::CombatPolicy {
                    scope: *scope,
                    op: op
                        .try_map(|_| Ok::<_, ()>(model::Expr::Const(model::Value::Float(value))))
                        .unwrap(),
                },
                other => panic!("unexpected {other:?}"),
            };
            assert!(live.validate().is_err());
        }
        graph.transitions[0].actions = vec![make(Expr::Const(ConstValue::Int(1)))];
        assert!(!validate(&graph).is_empty());
    }
}

#[test]
fn damage_delta_clamps_healing_and_lifetime_queue_is_bounded_and_identity_scoped() {
    use super::damage_commands::{damage_delta, Lease, Queue, Touched};
    use super::phase_commands::Command;
    assert_eq!(damage_delta(5.0, -10.0).unwrap(), -5.0);
    assert_eq!(damage_delta(5.0, 10.0).unwrap(), 10.0);
    assert_eq!(damage_delta(0.0, -999.0).unwrap(), 0.0);
    for (current, amount) in [
        (f32::NAN, 1.0),
        (-1.0, 1.0),
        (1.0, f32::INFINITY),
        (1.0, 1000.0),
    ] {
        assert!(damage_delta(current, amount).is_err());
    }
    let old = Lease {
        status: 10,
        revision: 1,
    };
    let mut queue = Queue::default();
    for boid in 0..8 {
        queue
            .enqueue(boid, "robot", boid, old, status(CombatOp::SuperArmor))
            .unwrap();
    }
    assert!(queue.preflight(8, 8, 1).is_err());
    assert!(queue
        .enqueue(8, "robot", 8, old, status(CombatOp::SuperArmor))
        .is_err());
    let mut actor = queue.take(0).unwrap();
    assert!(queue.take(0).is_none());
    assert_eq!(actor.prepare(old).1, vec![status(CombatOp::SuperArmor)]);
    actor.touched.record(status(CombatOp::SuperArmor));
    actor.touched.record(status(CombatOp::DamageTaken(0.5)));
    actor.touched.record(status(CombatOp::KnockbackTaken(2.0)));
    actor.touched.record(status(CombatOp::KnockbackDealt(2.0)));
    queue.retain(actor).unwrap();
    for next in [Lease { status: 11, ..old }, Lease { revision: 2, ..old }] {
        queue
            .enqueue(0, "robot", 0, next, status(CombatOp::Armor(5.0)))
            .unwrap();
        let mut actor = queue.take(0).unwrap();
        let (reset, commands) = actor.prepare(next);
        let mut expected = Touched::default();
        for op in [
            CombatOp::DamageTaken(0.5),
            CombatOp::KnockbackTaken(2.0),
            CombatOp::KnockbackDealt(2.0),
            CombatOp::SuperArmor,
        ] {
            expected.combat[op.channel() as usize] = Some((PolicyScope::Status, op));
        }
        assert_eq!(reset, expected);
        assert_eq!(commands, vec![status(CombatOp::Armor(5.0))]);
        actor.touched = reset;
        actor.lease = old;
        queue.retain(actor).unwrap();
    }
    // An entirely new stock/accessor identity inherits no active modifiers.
    queue
        .enqueue(0, "robot", 100, old, Command::AddDamage(-1.0))
        .unwrap();
    let mut actor = queue.take(0).unwrap();
    assert_eq!(actor.touched, Touched::default());
    for command in [
        status(CombatOp::DamageTaken(0.5)),
        status(CombatOp::DamageTaken(1.0)),
        status(CombatOp::KnockbackTaken(2.0)),
        status(CombatOp::KnockbackTaken(1.0)),
        status(CombatOp::KnockbackDealt(2.0)),
        status(CombatOp::KnockbackDealt(1.0)),
        status(CombatOp::SuperArmor),
        status(CombatOp::Clear(CombatChannel::Armor)),
    ] {
        actor.touched.record(command);
    }
    assert_eq!(actor.touched, Touched::default());
    // Old queued actions do not survive a status edge or graph replacement.
    assert!(actor.prepare(Lease { revision: 2, ..old }).1.is_empty());
    assert!(queue.preflight(0, 100, 65).is_err());
    for _ in 0..64 {
        queue
            .enqueue(0, "robot", 100, old, Command::AddDamage(1.0))
            .unwrap();
    }
    assert!(queue
        .enqueue(0, "robot", 100, old, Command::AddDamage(1.0))
        .is_err());
}

#[test]
fn damage_percent_formulas_collect_exact_owner_reads_and_work_on_stock_hooks() {
    let mut graph = fixture();
    let damage = NativeInput::Fighter {
        value: FighterValue::Damage,
        fighter: "robot".into(),
    };
    graph.transitions[0].on_frame = false;
    graph.states.push(StateDef {
        status: None,
        name: "done".into(),
    });
    graph.transitions[0].to = "done".into();
    graph.emission_plan.bindings[0].hook = hooks::HookName::OnStockStart;
    graph.transitions[0].actions = vec![Action::AddDamage {
        amount: Expr::Mul(
            Box::new(Expr::NativeFloat(damage.clone())),
            Box::new(Expr::Const(ConstValue::Float(-0.5))),
        ),
    }];
    assert_eq!(collect_native_inputs(&graph), vec![damage.clone()]);
    let live = program(&graph);
    assert_eq!(live.native_inputs, vec![damage.clone()]);
    assert!(!validate_for_fighter("mario", &graph).is_empty());
    let snapshot = NativeSnapshot::from_observations(
        &[damage],
        &[super::native_inputs::NativeObservation::Float(40.0)],
    )
    .unwrap();
    let result = live
        .step_with_snapshot(
            &mut live.initial_state(),
            model::HookName::OnStockStart,
            &snapshot,
        )
        .unwrap();
    assert_eq!(
        result.engine_actions,
        vec![model::Action::AddDamage {
            amount: model::Expr::Const(model::Value::Float(-20.0))
        }]
    );
    assert!(live
        .step(&mut live.initial_state(), model::HookName::OnStockStart)
        .is_err());
}

fn held_graph() -> BehaviorGraph {
    let mut graph = fixture();
    graph.transitions[0].guard = Predicate::NativeFlag(NativeInput::Fighter {
        value: FighterValue::HoldingOpponent,
        fighter: "robot".into(),
    });
    graph.transitions[0].actions = vec![
        Action::Set {
            var: "x".into(),
            value: Expr::NativeFloat(NativeInput::Fighter {
                value: FighterValue::HeldOpponentWeight,
                fighter: "robot".into(),
            }),
        },
        Action::AddHeldOpponentDamage {
            amount: Expr::Mul(
                Box::new(Expr::NativeFloat(NativeInput::Fighter {
                    value: FighterValue::HeldOpponentDamage,
                    fighter: "robot".into(),
                })),
                Box::new(Expr::Const(ConstValue::Float(0.1))),
            ),
        },
        Action::ReleaseHeldOpponent,
    ];
    graph
}
fn held_snapshot(held: bool, damage: f32, weight: f32) -> NativeSnapshot {
    use super::native_inputs::NativeObservation;
    NativeSnapshot::from_observations(
        &[
            NativeInput::Fighter {
                value: FighterValue::HoldingOpponent,
                fighter: "robot".into(),
            },
            NativeInput::Fighter {
                value: FighterValue::HeldOpponentDamage,
                fighter: "robot".into(),
            },
            NativeInput::Fighter {
                value: FighterValue::HeldOpponentWeight,
                fighter: "robot".into(),
            },
        ],
        &[
            NativeObservation::Flag(held),
            NativeObservation::Float(damage),
            NativeObservation::Float(weight),
        ],
    )
    .unwrap()
}

#[test]
fn held_opponent_reads_and_actions_resolve_only_from_fresh_typed_owner_snapshot() {
    let graph = held_graph();
    assert!(validate_for_fighter("robot", &graph).is_empty());
    assert!(!validate_for_fighter("mario", &graph).is_empty());
    let good = program(&graph);
    assert!(good
        .required_native_features()
        .contains(super::native_inputs::HELD_OPPONENT_FEATURE));
    let result = good
        .step_frame_with_snapshot(&mut good.initial_state(), &held_snapshot(true, 80.0, 106.0))
        .unwrap();
    assert_eq!(result.after.values["x"], model::Value::Float(106.0));
    assert_eq!(
        result.engine_actions,
        vec![
            model::Action::AddHeldOpponentDamage {
                amount: model::Expr::Const(model::Value::Float(8.0))
            },
            model::Action::ReleaseHeldOpponent,
        ]
    );
    assert!(good
        .step_frame_with_snapshot(&mut good.initial_state(), &held_snapshot(false, 0.0, 0.0))
        .unwrap()
        .engine_actions
        .is_empty());
    assert!(good
        .step_frame_with_snapshot(&mut good.initial_state(), &NativeSnapshot::empty())
        .is_err());
    let controller =
        super::controller::ControllerAction::try_from(&graph.transitions[0].actions[1]);
    assert!(controller.is_ok());
}

#[test]
fn held_opponent_actions_reject_unbounded_values_and_wrong_roots_in_both_models() {
    for amount in [-0.1, 1000.0, f32::NAN, f32::INFINITY] {
        let mut graph = fixture();
        graph.transitions[0].actions = vec![Action::AddHeldOpponentDamage {
            amount: Expr::Const(ConstValue::Float(amount)),
        }];
        assert!(!validate_for_fighter("robot", &graph).is_empty());
        assert!(live_protocol::program_from_graph("robot", &graph).is_err());
        let mut wire = program(&fixture());
        wire.transitions[0].actions = vec![model::Action::AddHeldOpponentDamage {
            amount: model::Expr::Const(model::Value::Float(amount)),
        }];
        assert!(wire.validate().is_err());
    }
    for action in [
        Action::AddHeldOpponentDamage {
            amount: Expr::Const(ConstValue::Int(1)),
        },
        Action::ReleaseHeldOpponent,
    ] {
        let mut graph = fixture();
        graph.transitions[0].actions = vec![action];
        graph.emission_plan.bindings[0].hook = hooks::HookName::OnDealDamage;
        graph.transitions[0].on_frame = false;
        assert!(!validate_for_fighter("robot", &graph).is_empty());
        assert!(live_protocol::program_from_graph("robot", &graph).is_err());
    }
    let mut graph = fixture();
    graph.transitions[0].actions = vec![Action::AddHeldOpponentDamage {
        amount: Expr::Var("x".into()),
    }];
    let good = program(&graph);
    let mut state = good.initial_state();
    state.values.insert("x".into(), model::Value::Float(1000.0));
    assert!(good.step_frame(&mut state).is_err());
}

#[test]
fn held_opponent_selectors_are_closed_read_only_and_round_trip_with_static_live_parity() {
    for (source, name, kind) in [
        (
            NativeInput::Fighter {
                value: FighterValue::HoldingOpponent,
                fighter: "robot".into(),
            },
            "holding_opponent",
            NativeKind::Flag,
        ),
        (
            NativeInput::Fighter {
                value: FighterValue::HeldOpponentDamage,
                fighter: "robot".into(),
            },
            "held_opponent_damage",
            NativeKind::Float,
        ),
        (
            NativeInput::Fighter {
                value: FighterValue::HeldOpponentWeight,
                fighter: "robot".into(),
            },
            "held_opponent_weight",
            NativeKind::Float,
        ),
        (
            NativeInput::Fighter {
                value: FighterValue::HeldOpponentEscapeTime,
                fighter: "robot".into(),
            },
            "held_opponent_escape_time",
            NativeKind::Float,
        ),
    ] {
        assert_eq!(source.kind(), Some(kind));
        assert_eq!(source.reference(), format!("{name}:robot"));
        assert_eq!(
            serde_json::from_str::<NativeInput>(&serde_json::to_string(&source).unwrap()).unwrap(),
            source
        );
        assert!(source.validate_write("robot").is_err());
        assert!(!source.valid_for("mario"));
        assert!(NativeInput::catalog_for("robot", kind).contains(&source));
        for bad in [
            format!("{name}:invented"),
            format!("{name}:robot:1"),
            format!("{name}:0"),
        ] {
            let decoded: NativeInput = serde_json::from_value(serde_json::json!(bad)).unwrap();
            assert!(!decoded.is_supported());
        }
    }
    let mut read_only = held_graph();
    read_only.transitions[0].actions.truncate(1);
    assert!(super::behavior::requires_controller_runtime(&read_only));
    let read_only_source = adapter::render(&BTreeMap::from([("robot".into(), read_only)])).unwrap();
    assert!(read_only_source
        .source
        .contains("VISIONARY_STATIC_CONTROLLER_BUNDLE"));
    for source in [
        NativeInput::Fighter {
            value: FighterValue::HeldOpponentDamage,
            fighter: "robot".into(),
        },
        NativeInput::Fighter {
            value: FighterValue::HeldOpponentWeight,
            fighter: "robot".into(),
        },
    ] {
        use super::native_inputs::NativeObservation;
        for invalid in [-1.0, f32::NAN, f32::INFINITY, 1_000_001.0] {
            assert!(NativeSnapshot::from_observations(
                std::slice::from_ref(&source),
                &[NativeObservation::Float(invalid)]
            )
            .is_err());
        }
        assert!(
            NativeSnapshot::from_observations(&[source], &[NativeObservation::Flag(true)]).is_err()
        );
    }
    let graph = held_graph();
    let decoded: BehaviorGraph =
        serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
    assert_eq!(decoded, graph);
    let rendered = adapter::render(&BTreeMap::from([("robot".into(), graph.clone())])).unwrap();
    assert!(rendered.source.contains("AddHeldOpponentDamage"));
    assert!(rendered.source.contains("ReleaseHeldOpponent"));
    let wire = program(&graph);
    assert_eq!(
        serde_json::from_str::<model::LiveProgram>(&serde_json::to_string(&wire).unwrap()).unwrap(),
        wire
    );
    let line = rendered
        .source
        .lines()
        .find(|line| line.starts_with("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = "))
        .unwrap();
    let encoded: String = serde_json::from_str(
        line.strip_prefix("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = ")
            .unwrap()
            .strip_suffix(';')
            .unwrap(),
    )
    .unwrap();
    let bundle = super::controller_protocol::decode_request(&encoded).unwrap();
    assert_eq!(bundle.charts[0].program, wire);
}

#[test]
fn held_opponent_plugin_uses_capture_child_and_main_delivery_with_exact_identity_reproofs() {
    let runtime = include_str!("../../plugins/slight_replica/src/slight/gimmicks/grab_control.rs");
    for api in [
        "LinkModule::get_node_object_id(module, *LINK_NO_CAPTURE)",
        "Identity::from_live_fighter",
        "sv_battle_object::is_active",
        "sv_battle_object::category",
        "DamageModule::damage(module, 0)",
        "DamageModule::add_damage(module, delta, 0)",
        "WorkModule::get_param_float(module, hash40(\"weight\"), 0)",
        "CatchModule::catch_cut(",
        "pending.check(",
        "let commands = QUEUE.lock().take(boid)",
        "grab command skipped",
    ] {
        assert!(runtime.contains(api), "missing {api}");
    }
    let main = include_str!("../../plugins/slight_replica/src/slight/agent_extender/mod.rs");
    assert!(main.contains("gimmicks::grab_control::flush(boid)"));
    let phase = include_str!("../../plugins/slight_replica/src/slight/gimmicks/phase_control.rs");
    assert!(phase.contains("super::grab_control::enqueue(boid, fighter, expected, command)"));
    let sampler = include_str!("../../plugins/slight_replica/src/slight/gimmicks/native_values.rs");
    assert!(sampler.contains("super::grab_control::sample(boid, fighter, expected, source)?"));
}

#[test]
fn stock_formulas_and_conditions_use_the_shared_static_and_live_controller() {
    let mut graph = fixture();
    graph.transitions[0].guard = Predicate::Compare {
        op: CmpOp::Lt,
        left: Expr::NativeInt(NativeInput::Fighter {
            value: FighterValue::StocksRemaining,
            fighter: "robot".into(),
        }),
        right: Expr::NativeInt(NativeInput::Fighter {
            value: FighterValue::OpponentStocksRemaining,
            fighter: "robot".into(),
        }),
    };
    graph.transitions[0].actions = vec![Action::Set {
        var: "n".into(),
        value: Expr::NativeInt(NativeInput::Fighter {
            value: FighterValue::StocksRemaining,
            fighter: "robot".into(),
        }),
    }];
    assert!(super::behavior::requires_controller_runtime(&graph));
    let rendered = adapter::render(&BTreeMap::from([("robot".into(), graph.clone())])).unwrap();
    let line = rendered
        .source
        .lines()
        .find(|line| line.starts_with("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = "))
        .unwrap();
    let encoded: String = serde_json::from_str(
        line.strip_prefix("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = ")
            .unwrap()
            .strip_suffix(';')
            .unwrap(),
    )
    .unwrap();
    let bundle = super::controller_protocol::decode_request(&encoded).unwrap();
    assert_eq!(bundle.charts[0].program, program(&graph));
}

#[test]
fn capture_events_round_trip_and_route_only_the_selected_hook() {
    for (hook, live, label) in [
        (
            hooks::HookName::OnGrab,
            model::HookName::OnGrab,
            "Grabs opponent",
        ),
        (
            hooks::HookName::OnThrow,
            model::HookName::OnThrow,
            "Releases throw",
        ),
        (
            hooks::HookName::OnGrabbed,
            model::HookName::OnGrabbed,
            "Gets grabbed",
        ),
    ] {
        assert!(hooks::resolve(hook.name()).is_ok());
        assert!(super::graph_edit::direct_hooks().contains(&hook));
        assert_eq!(super::graph_view::hook_label(hook), label);
        let mut graph = fixture();
        graph.emission_plan.bindings[0].hook = hook;
        graph.transitions[0].actions = vec![Action::Add {
            var: "n".into(),
            value: Expr::Const(ConstValue::Int(1)),
        }];
        let json = serde_json::to_string(&graph).unwrap();
        assert!(json.contains(hook.name()));
        let mut graph: BehaviorGraph = serde_json::from_str(&json).unwrap();
        let good = program(&graph);
        assert!(good
            .required_native_features()
            .contains(super::native_inputs::GRAB_EVENTS_FEATURE));
        let mut state = good.initial_state();
        good.step_with_snapshot(
            &mut state,
            model::HookName::OnChargeTick,
            &NativeSnapshot::empty(),
        )
        .unwrap();
        assert_eq!(state.values["n"], model::Value::Int(0));
        good.step_with_snapshot(&mut state, live, &NativeSnapshot::empty())
            .unwrap();
        assert_eq!(state.values["n"], model::Value::Int(1));
        let rendered = adapter::render(&BTreeMap::from([("robot".into(), graph.clone())])).unwrap();
        assert!(rendered.source.contains(hook.name()));
        // Throw direction uses the same fresh motion snapshot as frame guards.
        graph_motion_guard(&mut graph, live);
    }
}

fn graph_motion_guard(graph: &mut BehaviorGraph, hook: model::HookName) {
    graph.transitions[0].guard = Predicate::InMotion {
        motion: "throw_f".into(),
    };
    let good = program(graph);
    let mut snapshot = NativeSnapshot::empty();
    snapshot.phase = Some(super::native_inputs::PhaseInput {
        fighter: "robot".into(),
        status: 0,
        status_frame: 5,
        motion_frame: 5.0,
        motion: super::native_inputs::hash40("throw_f"),
        situation: Some(super::native_inputs::SituationKind::Ground),
    });
    let mut state = good.initial_state();
    assert!(good
        .step_with_snapshot(&mut state, hook, &NativeSnapshot::empty())
        .is_err());
    good.step_with_snapshot(&mut state, hook, &snapshot)
        .unwrap();
    assert_eq!(state.values["n"], model::Value::Int(1));
    snapshot.phase.as_mut().unwrap().motion = super::native_inputs::hash40("throw_b");
    good.step_with_snapshot(&mut state, hook, &snapshot)
        .unwrap();
    assert_eq!(state.values["n"], model::Value::Int(1));
    graph.transitions[0].guard = Predicate::Button {
        button: InputButton::Attack,
        edge: ButtonEdge::Pressed,
    };
    assert!(live_protocol::program_from_graph("robot", graph).is_err());
    let mut bad = good;
    bad.transitions[0].guard = model::Predicate::Button {
        button: InputButton::Attack,
        edge: ButtonEdge::Pressed,
    };
    assert!(bad.validate().is_err());
}

#[test]
fn capture_edges_fire_once_and_exclude_escape_and_interrupted_throw() {
    use super::native_inputs::CaptureState;
    let idle = CaptureState::default();
    let held = CaptureState {
        holding: true,
        ..idle
    };
    let throwing = CaptureState {
        throwing: true,
        ..held
    };
    let released = CaptureState {
        throwing: true,
        ..idle
    };
    let captured = CaptureState {
        captured: true,
        ..idle
    };
    assert_eq!(idle.edges(held), [true, false, false]);
    assert_eq!(held.edges(held), [false; 3]);
    assert_eq!(held.edges(throwing), [false; 3]);
    assert_eq!(throwing.edges(released), [false, true, false]);
    assert_eq!(released.edges(released), [false; 3]);
    assert_eq!(held.edges(idle), [false; 3]);
    assert_eq!(throwing.edges(idle), [false; 3]);
    assert_eq!(idle.edges(captured), [false, false, true]);
    assert_eq!(captured.edges(captured), [false; 3]);
    assert_eq!(captured.edges(idle), [false; 3]);
    assert_eq!(idle.edges(captured), [false, false, true]);
}

#[test]
fn movement_actions_round_trip_validate_and_dispatch_on_frame_only() {
    use super::controller::{ControllerAction, ControllerActionKind};
    use super::native_inputs::{Facing, MovementAction as M, MOVEMENT_FEATURE};
    let operations = [
        M::SetSpeed {
            x: None,
            y: Some(0.0),
        },
        M::SetSpeed {
            x: Some(-10.0),
            y: Some(10.0),
        },
        M::SetGravity { enabled: false },
        M::SetGravity { enabled: true },
        M::MoveBy {
            dx: 30.0,
            dy: -50.0,
        },
        M::SetFacing(Facing::Left),
        M::SetFacing(Facing::Right),
        M::SetFacing(Facing::Turn),
        M::SetFacing(Facing::Opponent),
    ];
    for op in operations {
        let action = ControllerAction::Movement(op);
        let ir = action.to_ir("robot").unwrap();
        let json = serde_json::to_string(&ir).unwrap();
        let decoded: super::behavior::Action = serde_json::from_str(&json).unwrap();
        assert_eq!(ControllerAction::try_from(&decoded).unwrap(), action);
        let mut graph = fixture();
        graph.transitions[0].actions = vec![ir];
        assert!(validate(&graph).is_empty());
        let live = program(&graph);
        assert!(live.required_native_features().contains(MOVEMENT_FEATURE));
        let result = live.step_frame(&mut live.initial_state()).unwrap();
        assert_eq!(result.engine_actions, vec![model::Action::Movement(op)]);
        assert_eq!(
            super::phase_commands::resolve(&result.engine_actions[0]).unwrap(),
            super::phase_commands::Command::Movement(op)
        );
        let rendered = adapter::render(&BTreeMap::from([("robot".into(), graph.clone())])).unwrap();
        assert!(rendered.source.contains("Movement"));
        graph.transitions[0].on_frame = false;
        assert!(!validate(&graph).is_empty());
        assert!(live_protocol::program_from_graph("robot", &graph).is_err());
    }
    for op in [
        M::SetSpeed { x: None, y: None },
        M::SetSpeed {
            x: Some(10.01),
            y: None,
        },
        M::SetSpeed {
            x: None,
            y: Some(f32::NAN),
        },
        M::MoveBy { dx: 50.01, dy: 0.0 },
        M::MoveBy {
            dx: 0.0,
            dy: f32::INFINITY,
        },
    ] {
        assert!(ControllerAction::Movement(op).to_ir("robot").is_err());
        let mut graph = fixture();
        graph.transitions[0].actions = vec![Action::Movement(op)];
        assert!(!validate(&graph).is_empty());
        assert!(live_protocol::program_from_graph("robot", &graph).is_err());
        assert!(super::phase_commands::resolve(&model::Action::Movement(op)).is_err());
    }
    for kind in [
        ControllerActionKind::SetSpeed,
        ControllerActionKind::SetGravity,
        ControllerActionKind::MoveBy,
        ControllerActionKind::SetFacing,
    ] {
        assert_eq!(kind.group(), "Movement");
    }
}

#[test]
fn movement_values_are_owned_read_only_float_snapshots() {
    use super::native_inputs::{NativeObservation, MOVEMENT_FEATURE};
    for source in [
        NativeInput::Fighter {
            value: FighterValue::SpeedX,
            fighter: "robot".into(),
        },
        NativeInput::Fighter {
            value: FighterValue::SpeedY,
            fighter: "robot".into(),
        },
        NativeInput::Fighter {
            value: FighterValue::PositionX,
            fighter: "robot".into(),
        },
        NativeInput::Fighter {
            value: FighterValue::PositionY,
            fighter: "robot".into(),
        },
        NativeInput::Fighter {
            value: FighterValue::Facing,
            fighter: "robot".into(),
        },
    ] {
        assert!(source.valid_for("robot"));
        assert!(!source.valid_for("cloud"));
        assert_eq!(source.kind(), Some(NativeKind::Float));
        assert_eq!(source.required_feature(), Some(MOVEMENT_FEATURE));
        assert!(source.validate_write("robot").is_err());
        assert!(NativeInput::catalog_for("robot", NativeKind::Float).contains(&source));
        let encoded = serde_json::to_string(&source).unwrap();
        assert_eq!(
            serde_json::from_str::<NativeInput>(&encoded).unwrap(),
            source
        );
        let mut graph = fixture();
        graph.transitions[0].guard = Predicate::Compare {
            op: CmpOp::Lt,
            left: Expr::NativeFloat(source.clone()),
            right: Expr::Const(ConstValue::Float(1.0)),
        };
        graph.transitions[0].actions = vec![Action::Add {
            var: "x".into(),
            value: Expr::Const(ConstValue::Float(1.0)),
        }];
        let live = program(&graph);
        let snapshot = NativeSnapshot::from_observations(
            std::slice::from_ref(&source),
            &[NativeObservation::Float(-1.0)],
        )
        .unwrap();
        let mut state = live.initial_state();
        assert!(live.step_frame_with_snapshot(&mut state, &snapshot).is_ok());
        assert!(NativeSnapshot::from_observations(
            std::slice::from_ref(&source),
            &[NativeObservation::Float(f32::NAN)]
        )
        .is_err());
        let unknown = encoded.replace("robot", "not_a_fighter");
        assert!(matches!(
            serde_json::from_str::<NativeInput>(&unknown).unwrap(),
            NativeInput::Unsupported(_)
        ));
    }
}

#[test]
fn hero_menu_spell_is_brave_only_and_selects_closed_slots() {
    use super::hero_menu_commands::FEATURE;
    use super::native_inputs::NativeObservation;

    let source = NativeInput::HeroMenuSpell;
    assert!(source.valid_for("brave"));
    assert!(!source.valid_for("mario"));
    assert_eq!(source.kind(), Some(NativeKind::Int));
    assert_eq!(source.required_feature(), Some(FEATURE));
    assert!(NativeInput::catalog_for("brave", NativeKind::Int).contains(&source));
    assert!(NativeInput::catalog_for("mario", NativeKind::Int)
        .iter()
        .all(|candidate| candidate != &source));
    let encoded = serde_json::to_string(&source).unwrap();
    assert_eq!(
        serde_json::from_str::<NativeInput>(&encoded).unwrap(),
        source
    );

    let choose = super::controller::ControllerAction::ChooseHeroMenuSpell { slot: 2 };
    assert_eq!(
        choose.to_ir("brave").unwrap(),
        Action::ChooseHeroMenuSpell { slot: 2 }
    );
    assert!(choose.to_ir("mario").is_err());
    assert!(super::editor::action_available(
        "brave",
        &fixture(),
        &choose,
        false
    ));
    assert!(!super::editor::action_available(
        "mario",
        &fixture(),
        &choose,
        false
    ));
    assert!(!super::editor::action_available(
        "brave",
        &fixture(),
        &super::controller::ControllerAction::ChooseHeroMenuSpell { slot: 4 },
        false,
    ));

    let mut graph = fixture();
    graph.transitions[0].guard = Predicate::Compare {
        op: CmpOp::Eq,
        left: Expr::NativeInt(source.clone()),
        right: Expr::Const(ConstValue::Int(1)),
    };
    graph.transitions[0].actions = vec![Action::ChooseHeroMenuSpell { slot: 2 }];
    assert!(validate_for_fighter("brave", &graph).is_empty());
    assert!(!validate_for_fighter("mario", &graph).is_empty());
    let live = live_protocol::program_from_graph("brave", &graph).unwrap();
    assert!(live.required_native_features().contains(FEATURE));
    let snapshot = NativeSnapshot::from_observations(
        std::slice::from_ref(&source),
        &[NativeObservation::Int(1)],
    )
    .unwrap();
    let result = live
        .step_frame_with_snapshot(&mut live.initial_state(), &snapshot)
        .unwrap();
    assert_eq!(
        result.engine_actions,
        vec![model::Action::ChooseHeroMenuSpell { slot: 2 }]
    );
    let mut out_of_range = graph;
    out_of_range.transitions[0].actions = vec![Action::ChooseHeroMenuSpell { slot: 4 }];
    assert!(!validate_for_fighter("brave", &out_of_range).is_empty());
}

#[test]
fn partner_values_belong_to_ice_climbers_and_rosalina_only() {
    use super::native_inputs::{NativeObservation, PARTNER_FEATURE};

    for fighter in ["popo", "rosetta"] {
        let present = NativeInput::Fighter {
            value: FighterValue::PartnerPresent,
            fighter: fighter.into(),
        };
        let damage = NativeInput::Fighter {
            value: FighterValue::PartnerDamage,
            fighter: fighter.into(),
        };
        assert_eq!(present.kind(), Some(NativeKind::Flag));
        assert_eq!(damage.kind(), Some(NativeKind::Float));
        for (source, key) in [(&present, "partner_present"), (&damage, "partner_damage")] {
            assert!(source.valid_for(fighter));
            assert!(!source.valid_for("mario"));
            assert_eq!(source.required_feature(), Some(PARTNER_FEATURE));
            assert!(NativeInput::catalog_for(fighter, source.kind().unwrap()).contains(source));
            let encoded = serde_json::to_string(source).unwrap();
            assert_eq!(encoded, format!("\"{key}:{fighter}\""));
            assert_eq!(
                &serde_json::from_str::<NativeInput>(&encoded).unwrap(),
                source
            );
        }
    }
    for foreign in [
        "\"partner_present:mario\"",
        "\"partner_damage:nana\"",
        "\"partner_damage:popo:1\"",
    ] {
        assert!(matches!(
            serde_json::from_str::<NativeInput>(foreign).unwrap(),
            NativeInput::Unsupported(_)
        ));
    }
    for kind in [NativeKind::Flag, NativeKind::Float] {
        assert!(NativeInput::catalog_for("mario", kind)
            .iter()
            .all(|s| !matches!(
                s,
                NativeInput::Fighter {
                    value: FighterValue::PartnerPresent,
                    ..
                } | NativeInput::Fighter {
                    value: FighterValue::PartnerDamage,
                    ..
                }
            )));
    }

    // Popo gains 1% per frame while Nana is at 10% or more.
    let damage = NativeInput::Fighter {
        value: FighterValue::PartnerDamage,
        fighter: "popo".into(),
    };
    let mut graph = fixture();
    graph.transitions[0].guard = Predicate::Compare {
        op: CmpOp::Gte,
        left: Expr::NativeFloat(damage.clone()),
        right: Expr::Const(ConstValue::Float(10.0)),
    };
    graph.transitions[0].actions = vec![Action::Set {
        var: "x".into(),
        value: Expr::NativeFloat(damage.clone()),
    }];
    assert!(validate_for_fighter("popo", &graph).is_empty());
    assert!(!validate_for_fighter("mario", &graph).is_empty());
    // A read-only partner guard still needs the controller runtime that samples it.
    assert!(super::behavior::requires_controller_runtime(&graph));
    graph.transitions[0].actions = vec![Action::AddDamage {
        amount: Expr::Const(ConstValue::Float(1.0)),
    }];
    let live = live_protocol::program_from_graph("popo", &graph).unwrap();
    assert!(live.required_native_features().contains(PARTNER_FEATURE));
    for (value, fires) in [(12.0, true), (5.0, false)] {
        let snapshot = NativeSnapshot::from_observations(
            std::slice::from_ref(&damage),
            &[NativeObservation::Float(value)],
        )
        .unwrap();
        let result = live
            .step_frame_with_snapshot(&mut live.initial_state(), &snapshot)
            .unwrap();
        assert_eq!(
            result.engine_actions,
            if fires {
                vec![model::Action::AddDamage {
                    amount: model::Expr::Const(model::Value::Float(1.0)),
                }]
            } else {
                vec![]
            }
        );
    }
}

#[test]
fn movement_queue_expires_gravity_and_stale_commands() {
    use super::native_inputs::MovementAction as M;
    use super::{
        damage_commands::{Lease, Queue, Touched},
        phase_commands::Command,
    };
    let old = Lease {
        status: 10,
        revision: 1,
    };
    for next in [Lease { status: 11, ..old }, Lease { revision: 2, ..old }] {
        let mut queue = Queue::default();
        queue
            .enqueue(
                7,
                "robot",
                1u32,
                old,
                Command::Movement(M::SetGravity { enabled: false }),
            )
            .unwrap();
        let mut actor = queue.take(7).unwrap();
        for c in actor.prepare(old).1 {
            actor.touched.record(c);
        }
        assert!(actor.touched.gravity);
        queue.retain(actor).unwrap();
        queue
            .enqueue(
                7,
                "robot",
                1,
                old,
                Command::Movement(M::MoveBy { dx: 30.0, dy: 0.0 }),
            )
            .unwrap();
        let mut actor = queue.take(7).unwrap();
        let (restore, commands) = actor.prepare(next);
        assert!(restore.gravity);
        assert!(commands.is_empty());
        assert_eq!(actor.touched, Touched::default());
        queue.retain(actor).unwrap();
        assert!(queue.take(7).is_none());
    }
    let mut touched = Touched::default();
    touched.record(Command::Movement(M::SetGravity { enabled: false }));
    touched.record(Command::Movement(M::SetGravity { enabled: true }));
    assert!(!touched.gravity);
    let mut queue = Queue::default();
    for _ in 0..64 {
        queue
            .enqueue(
                7,
                "robot",
                1u32,
                old,
                Command::Movement(M::SetFacing(super::native_inputs::Facing::Turn)),
            )
            .unwrap();
    }
    assert!(queue.preflight(7, 1, 1).is_err());
    queue
        .enqueue(
            7,
            "robot",
            2,
            old,
            Command::Movement(M::SetGravity { enabled: true }),
        )
        .unwrap();
    let actor = queue.take(7).unwrap();
    assert_eq!(actor.identity, 2);
    assert_eq!(actor.pending.len(), 1);
    assert_eq!(actor.touched, Touched::default());
}

#[test]
fn item_presence_edges_reset_across_lifetimes_and_configuration() {
    use super::native_inputs::ItemState;
    let empty = ItemState {
        identity: (7, 1),
        revision: 2,
        holding: false,
    };
    let held = ItemState {
        holding: true,
        ..empty
    };
    assert_eq!(empty.edges(&held), [true, false]);
    assert_eq!(held.edges(&held), [false, false]);
    assert_eq!(held.edges(&empty), [false, true]);
    assert_eq!(empty.edges(&empty), [false, false]);
    for reset in [
        ItemState {
            identity: (7, 2),
            ..empty
        },
        ItemState {
            revision: 3,
            ..empty
        },
    ] {
        assert_eq!(held.edges(&reset), [false, false]);
        assert_eq!(reset.edges(&held), [false, false]);
    }
}

#[test]
fn item_hooks_and_kind_guard_round_trip_and_share_static_live_dispatch() {
    use super::native_inputs::{
        ItemAction, NativeObservation, HELD_ITEM_KIND_FEATURE, ITEM_EVENTS_FEATURE,
    };
    let input = NativeInput::Fighter {
        value: FighterValue::HeldItemKind,
        fighter: "robot".into(),
    };
    let item = ItemAction::item_kind("ITEM_KIND_BEAMSWORD").unwrap();
    assert_eq!(
        serde_json::to_string(&input).unwrap(),
        "\"held_item_kind:robot\""
    );
    assert_eq!(
        serde_json::from_str::<NativeInput>("\"held_item_kind:robot\"").unwrap(),
        input
    );
    assert_eq!(input.kind(), Some(NativeKind::Int));
    assert!(NativeInput::catalog_for("robot", NativeKind::Int).contains(&input));
    assert!(input.validate_write("robot").is_err());
    assert!(!input.valid_for("peach"));
    for bad in ["held_item_kind:unknown", "held_item_kind:robot:extra"] {
        assert!(!serde_json::from_str::<NativeInput>(&format!("\"{bad}\""))
            .unwrap()
            .is_supported());
    }
    for (hook, live) in [
        (hooks::HookName::OnItemPickup, model::HookName::OnItemPickup),
        (hooks::HookName::OnItemThrown, model::HookName::OnItemThrown),
    ] {
        assert!(hooks::resolve(hook.name()).is_ok());
        assert!(super::graph_edit::direct_hooks().contains(&hook));
        let mut graph = fixture();
        graph.emission_plan.bindings[0].hook = hook;
        graph.transitions[0].actions = vec![Action::Add {
            var: "n".into(),
            value: Expr::Const(ConstValue::Int(1)),
        }];
        assert!(super::behavior::requires_controller_runtime(&graph));
        assert!(program(&graph)
            .required_native_features()
            .contains(ITEM_EVENTS_FEATURE));
        // Pickup can branch on the current held kind. Release observes NONE.
        graph.transitions[0].guard = Predicate::Compare {
            op: CmpOp::Eq,
            left: Expr::NativeInt(input.clone()),
            right: Expr::Const(ConstValue::Int(item)),
        };
        let graph: BehaviorGraph =
            serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
        let good = program(&graph);
        assert!(good
            .required_native_features()
            .contains(ITEM_EVENTS_FEATURE));
        assert!(good
            .required_native_features()
            .contains(HELD_ITEM_KIND_FEATURE));
        let mut state = good.initial_state();
        let snapshot = NativeSnapshot::from_observations(
            std::slice::from_ref(&input),
            &[NativeObservation::Int(item)],
        )
        .unwrap();
        good.step_with_snapshot(&mut state, model::HookName::OnChargeTick, &snapshot)
            .unwrap();
        assert_eq!(state.values["n"], model::Value::Int(0));
        good.step_with_snapshot(&mut state, live, &snapshot)
            .unwrap();
        assert_eq!(state.values["n"], model::Value::Int(1));
        let other = NativeSnapshot::from_observations(
            std::slice::from_ref(&input),
            &[NativeObservation::Int(-1)],
        )
        .unwrap();
        good.step_with_snapshot(&mut state, live, &other).unwrap();
        assert_eq!(state.values["n"], model::Value::Int(1));
        assert!(good
            .step_with_snapshot(&mut state, live, &NativeSnapshot::empty())
            .is_err());
        let rendered = adapter::render(&BTreeMap::from([("robot".into(), graph)])).unwrap();
        let line = rendered
            .source
            .lines()
            .find(|line| line.starts_with("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = "))
            .unwrap();
        let encoded: String = serde_json::from_str(
            line.strip_prefix("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = ")
                .unwrap()
                .strip_suffix(';')
                .unwrap(),
        )
        .unwrap();
        let bundle = super::controller_protocol::decode_request(&encoded).unwrap();
        assert_eq!(bundle.charts[0].program, good);
    }
}

#[test]
fn resource_rows_keep_owner_bounds_reads_and_bundle_live_parity() {
    use super::resource_commands::{Command, Op, Writer, FEATURE, RESOURCES};
    let float = |v: f32| Expr::Const(ConstValue::Float(v));
    for entry in RESOURCES {
        let fighter = entry.fighter;
        let resource = super::resource_commands::Resource::named(entry.key).unwrap();
        let command = Command {
            op: Op::Add,
            resource,
            amount: float(1.0),
        };
        let action = super::controller::ControllerAction::Resource(command.clone());
        assert_eq!(
            action.to_ir(fighter).unwrap(),
            Action::Resource(command.clone())
        );
        assert!(action.to_ir("mario").is_err());
        assert!(super::editor::action_available(
            fighter,
            &fixture(),
            &action,
            false
        ));
        assert!(!super::editor::action_available(
            "mario",
            &fixture(),
            &action,
            false
        ));
        assert_eq!(
            super::controller::ControllerAction::try_from(&Action::Resource(command.clone()))
                .unwrap(),
            action
        );
        let mut graph = fixture();
        graph.transitions[0].actions = vec![Action::Resource(command.clone())];
        graph.transitions[0].guard = Predicate::Button {
            button: InputButton::Guard,
            edge: ButtonEdge::Held,
        };
        assert!(validate_for_fighter(fighter, &graph).is_empty());
        assert!(!validate_for_fighter("mario", &graph).is_empty());
        let program = live_protocol::program_from_graph(fighter, &graph).unwrap();
        assert!(program.required_native_features().contains(FEATURE));
        assert_eq!(
            program
                .required_native_features()
                .contains(super::native_inputs::CLOUD_LIMIT_ACTIONS_FEATURE),
            entry.writer == Writer::CloudLimit
        );
        let json = serde_json::to_string(&program).unwrap();
        let mut decoded: model::LiveProgram = serde_json::from_str(&json).unwrap();
        assert!(decoded.validate().is_ok());
        decoded.fighter = "mario".into();
        assert!(decoded.validate().is_err());
        // Resource actions run only in the controller bundle.
        let bundle = adapter::render(&BTreeMap::from([(fighter.into(), graph.clone())])).unwrap();
        assert!(bundle.source.contains(entry.key));
        let mut state = program.initial_state();
        let fired = program
            .step_with_snapshot(
                &mut state,
                model::HookName::OnChargeTick,
                &input_snapshot(InputButton::Guard, ButtonEdge::Held, true),
            )
            .unwrap();
        let resolved = command
            .try_map(|_| Ok::<_, ()>(model::Expr::Const(model::Value::Float(1.0))))
            .unwrap();
        assert_eq!(
            fired.fired[0].engine_actions,
            vec![model::Action::Resource(resolved)]
        );
        let input = entry.input();
        assert!(input.valid_for(fighter));
        assert_eq!(
            input.validate_write(fighter).is_err(),
            entry.writer != Writer::Work
        );
        for amount in [f32::NAN, f32::INFINITY, -entry.max - 1.0, entry.max + 1.0] {
            graph.transitions[0].actions = vec![Action::Resource(Command {
                op: Op::Set,
                resource,
                amount: float(amount),
            })];
            assert!(
                !validate_for_fighter(fighter, &graph).is_empty(),
                "{} accepted {amount}",
                entry.key
            );
        }
    }
    let half = Command {
        op: Op::Add,
        resource: super::resource_commands::Resource::named("steveDiamonds").unwrap(),
        amount: 0.5,
    };
    assert!(half.validate("pickel").is_err());
}

#[test]
fn held_throw_directions_survive_controller_wire_and_phase_resolution() {
    use super::native_inputs::ThrowDirection;
    for direction in [
        ThrowDirection::Forward,
        ThrowDirection::Back,
        ThrowDirection::Up,
        ThrowDirection::Down,
    ] {
        let mut graph = held_graph();
        graph.transitions[0].actions = vec![Action::ThrowHeldOpponent { direction }];
        assert!(validate_for_fighter("robot", &graph).is_empty());
        let controller =
            super::controller::ControllerAction::try_from(&graph.transitions[0].actions[0])
                .unwrap();
        assert_eq!(
            controller.to_ir("robot").unwrap(),
            graph.transitions[0].actions[0]
        );
        let rendered = adapter::render(&BTreeMap::from([("robot".into(), graph.clone())])).unwrap();
        assert!(rendered.source.contains("ThrowHeldOpponent"));
        let good = program(&graph);
        let wire = serde_json::to_string(&good).unwrap();
        let decoded: model::LiveProgram = serde_json::from_str(&wire).unwrap();
        let result = decoded
            .step_frame_with_snapshot(
                &mut decoded.initial_state(),
                &held_snapshot(true, 0.0, 100.0),
            )
            .unwrap();
        assert_eq!(
            result.engine_actions,
            vec![model::Action::ThrowHeldOpponent { direction }]
        );
        assert_eq!(
            super::phase_commands::resolve(&result.engine_actions[0]).unwrap(),
            super::phase_commands::Command::ThrowHeldOpponent(direction)
        );
        assert!(decoded
            .step_frame_with_snapshot(
                &mut decoded.initial_state(),
                &held_snapshot(false, 0.0, 0.0)
            )
            .unwrap()
            .engine_actions
            .is_empty());
    }
    assert!(serde_json::from_str::<ThrowDirection>("\"sideways\"").is_err());
}

#[test]
fn command_inputs_are_curated_owned_read_only_and_round_trip() {
    use super::native_inputs::{command_symbols, NativeObservation};
    for fighter in ["ryu", "ken", "dolly", "demon"] {
        for symbol in command_symbols(fighter) {
            let source = NativeInput::Command {
                fighter: fighter.into(),
                symbol: (*symbol).into(),
            };
            assert!(source.valid_for(fighter));
            assert!(!source.valid_for("mario"));
            assert!(source.validate_write(fighter).is_err());
            assert_eq!(source.kind(), Some(NativeKind::Flag));
            assert!(NativeInput::catalog_for(fighter, NativeKind::Flag).contains(&source));
            let json = serde_json::to_string(&source).unwrap();
            assert_eq!(serde_json::from_str::<NativeInput>(&json).unwrap(), source);
            for value in [false, true] {
                let snapshot = super::native_inputs::NativeSnapshot::from_observations(
                    std::slice::from_ref(&source),
                    &[NativeObservation::Flag(value)],
                )
                .unwrap();
                assert_eq!(snapshot.read_flag(&source).unwrap(), value);
            }
        }
    }
    for key in [
        "command:mario:FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N_COMMAND",
        "command:ryu:UNKNOWN",
        "command:ryu:FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N_COMMAND:extra",
    ] {
        let source: NativeInput = serde_json::from_value(serde_json::json!(key)).unwrap();
        assert!(!source.is_supported());
    }
}

#[test]
fn command_guard_requires_frame_binding_and_shares_controller_bundle() {
    let input = NativeInput::Command {
        fighter: "ryu".into(),
        symbol: "FIGHTER_PAD_CMD_CAT4_FLAG_SPECIAL_N_COMMAND".into(),
    };
    let mut graph = fixture();
    graph.transitions[0].guard = Predicate::NativeFlag(input.clone());
    graph.transitions[0].actions = vec![];
    graph.emission_plan.bindings[0].hook = hooks::HookName::OnChargeTick;
    graph.transitions[0].on_frame = true;
    assert!(super::behavior::requires_controller_runtime(&graph));
    graph.transitions[0].actions = vec![Action::Add {
        var: "n".into(),
        value: Expr::Const(ConstValue::Int(1)),
    }];
    let live = live_protocol::program_from_graph("ryu", &graph).unwrap();
    for (flag, expected) in [(false, 0), (true, 1)] {
        let snapshot = NativeSnapshot::from_observations(
            std::slice::from_ref(&input),
            &[super::native_inputs::NativeObservation::Flag(flag)],
        )
        .unwrap();
        let mut state = live.initial_state();
        live.step_with_snapshot(&mut state, model::HookName::OnChargeTick, &snapshot)
            .unwrap();
        assert_eq!(state.values["n"], model::Value::Int(expected));
    }
    let rendered = adapter::render(&BTreeMap::from([("ryu".into(), graph.clone())])).unwrap();
    assert!(rendered.source.contains("command:ryu:"));
    graph.emission_plan.bindings[0].hook = hooks::HookName::OnDealDamage;
    assert!(adapter::render(&BTreeMap::from([("ryu".into(), graph)])).is_err());
}

#[test]
fn form_switch_dispatch_runs_on_owner_main_before_native_flush() {
    let main = include_str!("../../plugins/slight_replica/src/slight/agent_extender/mod.rs");
    let callback = main
        .split("unsafe extern \"C\" fn fighter_line_main")
        .nth(1)
        .unwrap()
        .split("unsafe extern \"C\" fn weapon_line_main")
        .next()
        .unwrap();
    let observe = callback
        .find("live_behavior::observe_form(boid)")
        .expect("form handoff must run on its owning fighter Main");
    assert!(observe < callback.find("damage_control::flush(boid)").unwrap());
    let live = include_str!("../../plugins/slight_replica/src/slight/gimmicks/live_behavior.rs");
    assert!(
        !live
            .split("fn dispatch_inner(")
            .nth(1)
            .unwrap()
            .contains("controllers::observe_form(boid, fighter)"),
        "global ticks include inactive forms"
    );
}

/// The plugin crate's own tests never run, so its half of these primitives is pinned by reading it.
/// A real saved project (cloud-combined.json, copied
/// verbatim) keeps its native references through load, validation and save.
#[test]
fn saved_fixture_native_references_load_validate_and_save_unchanged() {
    let text = include_str!("../../tests/data/cloud-combined.json");
    let project: crate::mod_project::ModProjectFile = serde_json::from_str(text).unwrap();
    let saved: serde_json::Value = serde_json::from_str(text).unwrap();
    let mut inputs = Vec::new();
    for (id, chart) in &project.fighters["cloud"].controller_charts {
        let errors = validate_for_fighter("cloud", &chart.graph);
        assert!(errors.is_empty(), "{id}: {errors:?}");
        assert_eq!(
            serde_json::to_value(&chart.graph).unwrap()["transitions"],
            saved["fighters"]["cloud"]["controller_charts"][id]["graph"]["transitions"],
            "{id} changed on save"
        );
        inputs.extend(collect_native_inputs(&chart.graph));
    }
    assert!(inputs.contains(&NativeInput::fighter_value("cloud", FighterValue::SpeedX)));
    assert!(inputs.contains(&NativeInput::article_count(
        "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE"
    )));
    assert!(inputs.iter().all(NativeInput::is_supported), "{inputs:?}");
}

#[test]
fn object_speed_and_removal_round_trip_validate_and_resolve_on_frame() {
    use super::controller::ControllerAction;
    use super::native_inputs::{ObjectOp, OWNED_ARTICLES_FEATURE};
    const GYRO: &str = "FIGHTER_ROBOT_GENERATE_ARTICLE_GYRO";
    let speed = Action::Object {
        symbol: GYRO.into(),
        op: ObjectOp::SetSpeed {
            x: Expr::Var("x".into()),
            y: Expr::Const(ConstValue::Float(-1.5)),
        },
    };
    let remove = Action::Object {
        symbol: GYRO.into(),
        op: ObjectOp::Remove,
    };
    assert_eq!(
        serde_json::to_value(&remove).unwrap(),
        serde_json::json!({"Object":{"symbol":GYRO,"op":"Remove"}})
    );
    assert_eq!(
        serde_json::to_value(&speed).unwrap()["Object"]["op"]["SetSpeed"]["y"],
        serde_json::json!({"Const":{"Float":-1.5}})
    );
    for action in [&speed, &remove] {
        let typed = ControllerAction::try_from(action).unwrap();
        assert_eq!(&typed.to_ir("robot").unwrap(), action);
        assert!(typed.to_ir("cloud").is_err());
    }
    let mut graph = fixture();
    graph.transitions[0].actions = vec![speed, remove];
    assert!(
        validate_for_fighter("robot", &graph).is_empty(),
        "{:?}",
        validate_for_fighter("robot", &graph)
    );
    let live = program(&graph);
    assert!(live
        .required_native_features()
        .contains(OWNED_ARTICLES_FEATURE));
    let mut state = live.initial_state();
    state.values.insert("x".into(), model::Value::Float(3.0));
    let actions = live
        .step_frame_with_snapshot(&mut state, &NativeSnapshot::empty())
        .unwrap()
        .engine_actions;
    let float = |v| model::Expr::Const(model::Value::Float(v));
    assert_eq!(
        actions,
        vec![
            model::Action::Object {
                symbol: GYRO.into(),
                op: ObjectOp::SetSpeed {
                    x: float(3.0),
                    y: float(-1.5)
                }
            },
            model::Action::Object {
                symbol: GYRO.into(),
                op: ObjectOp::Remove
            },
        ]
    );
    // A runtime value outside ±10 stops the step instead of reaching the engine.
    state.values.insert("x".into(), model::Value::Float(11.0));
    assert!(live
        .step_frame_with_snapshot(&mut state, &NativeSnapshot::empty())
        .is_err());
    for bad in [
        Action::Object {
            symbol: GYRO.into(),
            op: ObjectOp::SetSpeed {
                x: Expr::Const(ConstValue::Float(11.0)),
                y: Expr::Const(ConstValue::Float(0.0)),
            },
        },
        Action::Object {
            symbol: GYRO.into(),
            op: ObjectOp::SetSpeed {
                x: Expr::Var("n".into()),
                y: Expr::Const(ConstValue::Float(0.0)),
            },
        },
        Action::Object {
            symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
            op: ObjectOp::Remove,
        },
    ] {
        let mut wrong = fixture();
        wrong.transitions[0].actions = vec![bad.clone()];
        assert!(
            !validate_for_fighter("robot", &wrong).is_empty(),
            "accepted {bad:?}"
        );
    }
}

fn policy(op: CombatOp<Expr>) -> Action {
    Action::CombatPolicy {
        scope: PolicyScope::Status,
        op,
    }
}
fn status(op: CombatOp<f32>) -> super::phase_commands::Command {
    super::phase_commands::Command::Combat(PolicyScope::Status, op)
}

#[test]
fn combat_policy_scopes_expire_on_their_own_boundary_and_old_saves_load() {
    use super::damage_commands::{Lease, Queue};
    use super::phase_commands::Command;
    let lease = Lease {
        status: 10,
        revision: 1,
    };
    let mut queue = Queue::default();
    queue
        .enqueue(
            0,
            "mario",
            0,
            lease,
            Command::Combat(PolicyScope::Status, CombatOp::SuperArmor),
        )
        .unwrap();
    let mut actor = queue.take(0).unwrap();
    for command in actor.prepare(lease).1 {
        actor.touched.record(command);
    }
    actor.touched.record(Command::Combat(
        PolicyScope::Frames(3),
        CombatOp::Intangible,
    ));
    actor.touched.record(Command::Combat(
        PolicyScope::UntilCleared,
        CombatOp::DamageDealt(2.0),
    ));
    let ended = |actor: &mut super::damage_commands::Actor<u32>, lease| {
        let reset = actor.prepare(lease).0;
        CombatChannel::ALL
            .into_iter()
            .filter(|c| reset.has(*c))
            .collect::<Vec<_>>()
    };
    // Frames count every boundary, across a status change; status scope ends at the edge,
    // and channels held past it are written again with their remaining frames.
    assert_eq!(ended(&mut actor, lease), vec![]);
    let next = Lease {
        status: 11,
        ..lease
    };
    let reset = actor.prepare(next);
    assert!(reset.0.has(CombatChannel::Armor) && !reset.0.has(CombatChannel::Hurtbox));
    assert_eq!(
        reset.1,
        vec![
            Command::Combat(PolicyScope::UntilCleared, CombatOp::DamageDealt(2.0)),
            Command::Combat(PolicyScope::Frames(1), CombatOp::Intangible),
        ]
    );
    assert_eq!(ended(&mut actor, next), vec![CombatChannel::Hurtbox]);
    for _ in 0..100 {
        assert_eq!(
            ended(
                &mut actor,
                Lease {
                    status: 12,
                    ..lease
                }
            ),
            vec![]
        );
    }
    assert!(actor.touched.has(CombatChannel::DamageDealt));
    actor.touched.record(Command::Combat(
        PolicyScope::Status,
        CombatOp::Clear(CombatChannel::DamageDealt),
    ));
    assert_eq!(actor.touched, super::damage_commands::Touched::default());
    // A controller change ends every scope, including until-cleared.
    actor.touched.record(Command::Combat(
        PolicyScope::UntilCleared,
        CombatOp::Invincible,
    ));
    assert_eq!(
        ended(
            &mut actor,
            Lease {
                revision: 2,
                ..lease
            }
        ),
        vec![CombatChannel::Hurtbox]
    );
    // Pre-CombatPolicy saves load as the status-scoped policy.
    for (legacy, op) in [
        (
            serde_json::json!({"SetDamageMultiplier": {"multiplier": {"Const": {"Float": 0.5}}}}),
            CombatOp::DamageTaken(Expr::Const(ConstValue::Float(0.5))),
        ),
        (
            serde_json::json!({"SetReactionMultiplier": {"multiplier": {"Const": {"Float": 2.0}}}}),
            CombatOp::KnockbackTaken(Expr::Const(ConstValue::Float(2.0))),
        ),
        (
            serde_json::json!({"SetArmor": {"threshold": {"Const": {"Float": 12.0}}}}),
            CombatOp::Armor(Expr::Const(ConstValue::Float(12.0))),
        ),
        (serde_json::json!("SetSuperArmor"), CombatOp::SuperArmor),
        (
            serde_json::json!("ClearArmor"),
            CombatOp::Clear(CombatChannel::Armor),
        ),
        (
            serde_json::json!({"ClearArmor": null}),
            CombatOp::Clear(CombatChannel::Armor),
        ),
        (
            serde_json::json!({"SetSuperArmor": null}),
            CombatOp::SuperArmor,
        ),
    ] {
        assert_eq!(
            serde_json::from_value::<Action>(legacy).unwrap(),
            policy(op)
        );
    }
    let saved = serde_json::to_value(Action::CombatPolicy {
        scope: PolicyScope::Frames(30),
        op: CombatOp::<Expr>::Intangible,
    })
    .unwrap();
    assert_eq!(
        saved,
        serde_json::json!({"CombatPolicy": {"scope": {"Frames": 30}, "op": "Intangible"}})
    );
    assert!(
        serde_json::from_value::<Action>(serde_json::json!({"SetArmor": {"limit": 1}})).is_err()
    );
    let mut graph = fixture();
    graph.transitions[0].actions = vec![Action::CombatPolicy {
        scope: PolicyScope::Frames(0),
        op: CombatOp::SuperArmor,
    }];
    assert!(!validate(&graph).is_empty());
    graph.transitions[0].actions = vec![Action::CombatPolicy {
        scope: PolicyScope::Frames(3601),
        op: CombatOp::DamageTaken(Expr::Const(ConstValue::Float(0.5))),
    }];
    assert!(!validate(&graph).is_empty());
}

#[test]
fn movement_stats_name_the_exact_getter_keys_and_scale_height_by_the_root() {
    use super::native_inputs::hash40;
    for (name, stat) in [
        ("run_speed_max", MovementStat::RunSpeed),
        ("air_speed_x_stable", MovementStat::AirSpeed),
        ("air_accel_y", MovementStat::Gravity),
        ("jump_speed_y", MovementStat::JumpHeight),
        ("mini_jump_speed_y", MovementStat::JumpHeight),
        ("jump_aerial_speed_y", MovementStat::JumpHeight),
        ("jump_y", MovementStat::JumpHeight),
        ("jump_initial_speed_y", MovementStat::JumpHeight),
        ("jump_initial_accel_y", MovementStat::JumpHeight),
    ] {
        assert_eq!(
            MovementStat::for_param(hash40(name), 0),
            Some(stat),
            "{name}"
        );
        // A group read with a child key is a different param with the same parent hash.
        assert_eq!(
            MovementStat::for_param(hash40(name), hash40("common")),
            None,
            "{name}"
        );
    }
    // Not stats: the two heights are read only when speeds are derived at load (main+0x6f21d0).
    for name in [
        "mini_jump_y",
        "jump_aerial_y",
        "air_speed_y_stable",
        "walk_speed_max",
    ] {
        assert_eq!(MovementStat::for_param(hash40(name), 0), None, "{name}");
    }
    let mut multipliers = [1.0; MovementStat::ALL.len()];
    multipliers[MovementStat::JumpHeight as usize] = 4.0;
    multipliers[MovementStat::Gravity as usize] = 0.5;
    assert_eq!(
        MovementStat::JumpHeight.read_factor(hash40("jump_aerial_speed_y"), &multipliers),
        2.0
    );
    assert_eq!(
        MovementStat::JumpHeight.read_factor(hash40("jump_y"), &multipliers),
        8.0
    );
    assert_eq!(
        MovementStat::Gravity.read_factor(hash40("air_accel_y"), &multipliers),
        0.5
    );
    for stat in MovementStat::ALL {
        assert_eq!(stat.channel().stat(), Some(stat));
        assert_eq!(CombatOp::Stat(stat, 1.5).channel(), stat.channel());
    }
    assert_eq!(CombatChannel::Armor.stat(), None);
}

/// Replays main+0x6d5030 for a full hop on Mario's fighter_param.prc values, with
/// jump_initial_frame 4 and the initial speed and accel derived as main+0x6f21d0 does.
/// Falling is left out: the apex comes first, and the terminal cap only limits descent.
#[test]
fn a_full_hop_apex_is_jump_y_times_height_over_gravity() {
    use super::native_inputs::hash40;
    let (jump_y, jump_initial_y, air_accel_y, frames) = (36.33f32, 19.9815f32, 0.087f32, 4.0f32);
    let after =
        air_accel_y * (2.0 * (jump_y - jump_initial_y) / air_accel_y).sqrt() + 0.5 * air_accel_y;
    let accel = (jump_initial_y - after * frames) / (frames * (frames + 1.0) / 2.0);
    let speed = accel * frames + after;
    let apex = |height: f32, gravity: f32| {
        let mut multipliers = [1.0; MovementStat::ALL.len()];
        multipliers[MovementStat::JumpHeight as usize] = height;
        multipliers[MovementStat::Gravity as usize] = gravity;
        let read = |name: &str, value: f32| match MovementStat::for_param(hash40(name), 0) {
            Some(stat) => value * stat.read_factor(hash40(name), &multipliers),
            None => value,
        };
        let (g, accel) = (
            read("air_accel_y", air_accel_y),
            read("jump_initial_accel_y", accel),
        );
        let (mut v, mut y) = (read("jump_initial_speed_y", speed) + accel.abs(), 0.0f32);
        for _ in 0..frames as u32 {
            v -= accel;
            y += v;
        }
        let climbed = y;
        v = g * (2.0 * (read("jump_y", jump_y) - climbed).max(0.0) / g).sqrt() + 0.5 * g;
        loop {
            v -= g;
            if v <= 0.0 {
                return y;
            }
            y += v;
        }
    };
    for (height, gravity) in [
        (1.0, 1.0),
        (2.0, 1.0),
        (1.0, 0.5),
        (1.0, 2.0),
        (2.0, 0.5),
        (0.25, 1.0),
        (10.0, 0.1),
    ] {
        let expected = jump_y * height / gravity;
        assert!(
            (apex(height, gravity) - expected).abs() < expected * 0.01 + 0.2,
            "{height} / {gravity}: {} != {expected}",
            apex(height, gravity)
        );
    }
}

#[test]
fn a_hidden_mesh_is_shown_again_when_the_hold_ends_but_not_by_a_status_change() {
    use super::damage_commands::{Lease, Queue, Touched};
    use super::phase_commands::Command;
    let mesh = |mesh, visible| Command::MeshVisibility { mesh, visible };
    let lease = Lease {
        status: 10,
        revision: 1,
    };
    let mut queue = Queue::default();
    queue.enqueue(0, "mario", 0, lease, mesh(7, false)).unwrap();
    let mut actor = queue.take(0).unwrap();
    for command in actor.prepare(lease).1 {
        actor.touched.record(command);
    }
    // A chart may send the same hide every frame without filling the slots.
    actor.touched.record(mesh(7, false));
    actor.touched.record(mesh(8, false));
    actor.touched.record(mesh(8, true));
    assert_eq!(actor.touched.hidden_meshes, [7, 0, 0, 0, 0, 0, 0, 0]);
    assert_ne!(
        actor.touched,
        Touched::default(),
        "the queue would drop the actor and forget the mesh"
    );
    assert_eq!(
        actor
            .prepare(Lease {
                status: 11,
                ..lease
            })
            .0
            .hidden_meshes,
        [0; 8]
    );
    let ended = actor
        .prepare(Lease {
            status: 11,
            revision: 2,
        })
        .0;
    assert_eq!(ended.hidden_meshes, [7, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(actor.touched, Touched::default());
}

#[test]
fn movement_stats_hold_across_status_changes_until_their_scope_ends() {
    use super::damage_commands::{Actor, Lease, Queue, Touched};
    use super::phase_commands::Command;
    let lease = Lease {
        status: 10,
        revision: 1,
    };
    let stat = |scope, stat, value| Command::Combat(scope, CombatOp::Stat(stat, value));
    let mut queue = Queue::default();
    queue
        .enqueue(
            0,
            "mario",
            0,
            lease,
            stat(PolicyScope::UntilCleared, MovementStat::RunSpeed, 2.0),
        )
        .unwrap();
    let mut actor = queue.take(0).unwrap();
    for command in actor.prepare(lease).1 {
        actor.touched.record(command);
    }
    let ended = |actor: &mut Actor<u32>, lease| {
        let reset = actor.prepare(lease).0;
        CombatChannel::ALL
            .into_iter()
            .filter(|c| reset.has(*c))
            .collect::<Vec<_>>()
    };
    // Until cleared outlives any number of status changes and stays recorded.
    for status in 11..40 {
        // Nothing to write again either: a status change never resets the overlay.
        let (reset, commands) = actor.prepare(Lease { status, ..lease });
        assert_eq!(reset, Touched::default());
        assert_eq!(commands, vec![]);
        assert!(actor.touched.has(CombatChannel::RunSpeed));
    }
    let lease = Lease {
        status: 40,
        ..lease
    };
    assert_eq!(ended(&mut actor, lease), vec![]);
    // Each stat is its own channel: a neutral value or a Clear releases only that one.
    actor
        .touched
        .record(stat(PolicyScope::UntilCleared, MovementStat::Gravity, 0.5));
    actor
        .touched
        .record(stat(PolicyScope::UntilCleared, MovementStat::Gravity, 1.0));
    assert!(
        !actor.touched.has(CombatChannel::Gravity) && actor.touched.has(CombatChannel::RunSpeed)
    );
    actor.touched.record(stat(
        PolicyScope::UntilCleared,
        MovementStat::JumpHeight,
        2.0,
    ));
    actor.touched.record(Command::Combat(
        PolicyScope::Status,
        CombatOp::Clear(CombatChannel::JumpHeight),
    ));
    assert!(
        !actor.touched.has(CombatChannel::JumpHeight) && actor.touched.has(CombatChannel::RunSpeed)
    );
    // A frame scope ends after its frames, and this status scope at the next status.
    actor
        .touched
        .record(stat(PolicyScope::Frames(2), MovementStat::AirSpeed, 2.0));
    actor
        .touched
        .record(stat(PolicyScope::Status, MovementStat::JumpHeight, 2.0));
    assert_eq!(ended(&mut actor, lease), vec![]);
    assert_eq!(ended(&mut actor, lease), vec![CombatChannel::AirSpeed]);
    assert_eq!(
        ended(
            &mut actor,
            Lease {
                status: 41,
                ..lease
            }
        ),
        vec![CombatChannel::JumpHeight]
    );
    // A controller change restores what is left.
    assert_eq!(
        ended(
            &mut actor,
            Lease {
                status: 41,
                revision: 2
            }
        ),
        vec![CombatChannel::RunSpeed]
    );
    assert_eq!(actor.touched, Touched::default());
}

#[test]
fn plugin_stat_overlay_is_read_by_both_getters_and_retired_with_the_stock() {
    let source =
        std::fs::read_to_string("plugins/slight_replica/src/slight/gimmicks/live_parameters.rs")
            .unwrap();
    let body = |name: &str| {
        let start = source.find(&format!("unsafe fn {name}(")).unwrap();
        let rest = &source[start..];
        rest[..rest.find("\n}\n").unwrap()].to_string()
    };
    for getter in ["hook_native_param_float", "hook_get_param_float"] {
        assert!(
            body(getter).contains("scale_stat("),
            "{getter} would ignore stats"
        );
    }
    // A lost stock never reaches a restore, because its identity no longer proves.
    let refresh = &source[source.find("pub fn refresh_identities()").unwrap()..];
    assert!(refresh[..refresh.find("\n}\n").unwrap()].contains("retire_stale_stats()"));
    let retire = &source[source.find("fn retire_stale_stats()").unwrap()..];
    let retire = &retire[..retire.find("\n}\n").unwrap()];
    assert!(
        retire.contains("stock_generation(&record)")
            && retire.contains("live == Some(scale.stock_generation)")
    );
    // set_stat holds the lock the getters try: logging under it would unscale their reads.
    let set = &source[source.find("fn set_stat(").unwrap()..];
    let set = &set[..set.find("\n}\n").unwrap()];
    assert!(set.find("drop(scales)").unwrap() < set.find("diag::note").unwrap());
}

#[test]
fn mesh_visibility_takes_a_lowercase_hash_name_through_every_layer() {
    use super::controller::ControllerAction;
    use super::phase_commands::{resolve, Command};
    let action = Action::MeshVisibility {
        mesh: "edge_wing".into(),
        visible: false,
    };
    assert_eq!(
        resolve(&action).unwrap(),
        Command::MeshVisibility {
            mesh: super::native_inputs::hash40("edge_wing"),
            visible: false
        }
    );
    assert!(
        super::damage_commands::is_damage(resolve(&action).unwrap()),
        "delivered on the owning Main line"
    );
    let typed = ControllerAction::try_from(&action).unwrap();
    assert_eq!(typed.to_ir("edge").unwrap(), action);
    let json = serde_json::to_string(&action).unwrap();
    assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), action);
    let mut graph = fixture();
    graph.transitions[0].actions = vec![action];
    assert!(validate(&graph).is_empty());
    assert!(program(&graph)
        .required_native_features()
        .contains(super::native_inputs::MOVEMENT_FEATURE));
    for bad in [
        "",
        "Edge_Wing",
        "edge_wing_VIS O",
        "edge-wing",
        &"m".repeat(65),
    ] {
        let action = Action::MeshVisibility {
            mesh: bad.into(),
            visible: true,
        };
        assert!(resolve(&action).is_err(), "{bad:?}");
        assert!(
            ControllerAction::try_from(&action)
                .unwrap()
                .to_ir("edge")
                .is_err(),
            "{bad:?}"
        );
        graph.transitions[0].actions = vec![action];
        assert!(!validate(&graph).is_empty(), "{bad:?}");
    }
}
