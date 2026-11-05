use super::{
    behavior::{
        self, Action, BehaviorGraph, CmpOp, ConstValue, Expr, Predicate, StateDef, Transition,
    },
    codegen::{EmissionPlan, HookBinding},
    controller::ControllerAction,
    controller_protocol as protocol,
    hooks::HookName,
    live_model as model, live_protocol,
};

const STATUS: &str = "FIGHTER_CLOUD_STATUS_KIND_SPECIAL_HI2";

fn graph(symbol: &str) -> BehaviorGraph {
    BehaviorGraph {
        custom_statuses: Default::default(),
        initial: "idle".into(),
        states: vec![
            StateDef {
                status: None,
                name: "idle".into(),
            },
            StateDef {
                status: None,
                name: "done".into(),
            },
        ],
        vars: vec![],
        transitions: vec![Transition {
            from: "idle".into(),
            to: "done".into(),
            on_frame: true,
            guard: Predicate::Compare {
                op: CmpOp::Eq,
                left: Expr::Const(ConstValue::Int(1)),
                right: Expr::Const(ConstValue::Int(1)),
            },
            actions: vec![Action::RequestFighterStatus {
                symbol: symbol.into(),
            }],
        }],
        emission_plan: EmissionPlan {
            bindings: vec![HookBinding {
                hook: HookName::OnChargeTick,
                article: None,
                transitions: vec![0],
            }],
            ..EmissionPlan::default()
        },
    }
}

#[test]
fn fighter_status_actions_are_strict_owned_symbols_in_saved_live_and_static_forms() {
    let graph = graph(STATUS);
    assert!(behavior::validate_for_fighter("cloud", &graph).is_empty());
    let json = serde_json::to_value(&graph).unwrap();
    assert_eq!(
        serde_json::from_value::<BehaviorGraph>(json).unwrap(),
        graph
    );
    let typed = ControllerAction::try_from(&graph.transitions[0].actions[0]).unwrap();
    assert_eq!(
        typed.to_ir("cloud").unwrap(),
        graph.transitions[0].actions[0]
    );
    assert!(typed.to_ir("mario").is_err());
    let artifact = super::adapter::render(&std::collections::BTreeMap::from([(
        "cloud".into(),
        graph.clone(),
    )]))
    .unwrap();
    assert!(artifact.source.contains(&format!(
        "Action::RequestFighterStatus {{ symbol: \"{STATUS}\" }}"
    )));
    for invalid in [
        serde_json::json!({"RequestFighterStatus": {"symbol": STATUS, "target": 1}}),
        serde_json::json!({"RequestFighterStatus": {}}),
        serde_json::json!({"RequestFighterStatus": {"symbol": 478}}),
    ] {
        assert!(serde_json::from_value::<Action>(invalid.clone()).is_err());
        assert!(serde_json::from_value::<model::Action>(invalid).is_err());
    }
    for symbol in [
        "FIGHTER_MARIO_STATUS_KIND_SPECIAL_LW_SHOOT",
        "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE",
    ] {
        let mut wrong = graph.clone();
        wrong.transitions[0].actions = vec![Action::RequestFighterStatus {
            symbol: symbol.into(),
        }];
        assert!(!behavior::validate_for_fighter("cloud", &wrong).is_empty());
        assert!(ControllerAction::RequestFighterStatus {
            symbol: symbol.into()
        }
        .to_ir("cloud")
        .is_err());
        let mut program = live_protocol::program_from_graph("cloud", &graph).unwrap();
        program.transitions[0].actions = vec![model::Action::RequestFighterStatus {
            symbol: symbol.into(),
        }];
        assert!(program.validate().is_err());
        assert!(live_protocol::validate_snapshot(&[program], &[]).is_err());
    }
}

#[test]
fn fighter_status_move_command_roundtrips_at_its_frame_and_checks_ownership() {
    use crate::data::{AcmdStmt, ExcuteStmt};
    let source = format!(
        r#"unsafe extern "C" fn game_status(agent: &mut L2CAgentBase) {{
        frame(agent.lua_state_agent, 12.0);
        if macros::is_excute(agent) {{
            StatusModule::change_status_request_from_script(agent.module_accessor, *{STATUS}, true);
        }}
    }}"#
    );
    let parsed = crate::acmd::parse_acmd_script(&source);
    let AcmdStmt::Excute(commands) = &parsed.stmts[1] else {
        panic!("missing frame command: {:?}", parsed.stmts)
    };
    let [ExcuteStmt::NativeCall(call)] = commands.as_slice() else {
        panic!("untyped status request: {commands:?}")
    };
    call.validate_for_fighter(Some("cloud")).unwrap();
    assert!(call.validate_for_fighter(Some("mario")).is_err());
    assert!(call.validate_for_fighter(None).is_err());
    let mut wrong = call.clone();
    wrong.arguments[0] = "*FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into();
    assert!(wrong.validate_for_fighter(Some("cloud")).is_err());
    wrong = call.clone();
    wrong.arguments[1] = "false".into();
    assert!(wrong.validate().is_err());
    let emitted = crate::acmd::preview_game_fn(&parsed, "status");
    assert!(emitted.contains(&call.source()));
    assert_eq!(
        serde_json::to_value(crate::acmd::parse_acmd_script(&emitted).stmts).unwrap(),
        serde_json::to_value(parsed.stmts).unwrap()
    );
}

#[test]
fn fighter_status_live_step_queues_the_owning_fighters_engine_action() {
    let program = live_protocol::program_from_graph("cloud", &graph(STATUS)).unwrap();
    let request = protocol::ControllerBundleRequest {
        version: protocol::REQUEST_VERSION,
        revision: 1,
        disabled_fighters: vec![],
        charts: vec![protocol::ControllerProgram {
            chart_id: "main".into(),
            program,
        }],
    };
    let decoded = protocol::decode_request(&protocol::encode_request(&request).unwrap()).unwrap();
    let program = &decoded.charts[0].program;
    let mut state = program.initial_state();
    let step = program.step_frame(&mut state).unwrap();
    assert_eq!(
        step.engine_actions,
        vec![model::Action::RequestFighterStatus {
            symbol: STATUS.into()
        }]
    );
    assert!(program
        .step_frame(&mut state)
        .unwrap()
        .engine_actions
        .is_empty());
}

#[test]
fn owned_status_roots_count_as_bound_and_teardown_after_exit_runs_nothing() {
    use super::status_model::Phase;
    let graph: BehaviorGraph =
        serde_json::from_str(include_str!("../../tests/data/mario_status_own.json")).unwrap();
    let program = live_protocol::program_from_graph("mario", &graph).unwrap();
    let mut state = program.initial_state();
    let mut snapshot = super::native_inputs::NativeSnapshot::default();
    snapshot.motion_ended = Some(true);
    for phase in [Phase::Pre, Phase::MainEnter] {
        assert_eq!(
            program
                .step_status_phase(&mut state, "cape", phase, &snapshot)
                .unwrap()
                .engine_actions
                .len(),
            1
        );
    }
    let exit = program
        .step_status_phase(&mut state, "cape", Phase::MainLoop, &snapshot)
        .unwrap();
    assert!(matches!(
        exit.engine_actions.as_slice(),
        [model::Action::Status(_)]
    ));
    assert_eq!(state.state, "idle");
    for phase in [Phase::End, Phase::Exit] {
        assert!(program
            .step_status_phase(&mut state, "cape", phase, &snapshot)
            .unwrap()
            .engine_actions
            .is_empty());
    }
    assert!(program
        .step_status_phase(&mut state, "cape", Phase::MainLoop, &snapshot)
        .is_err());
}

#[test]
fn situation_change_recipe_inherits_frame_once_and_uses_entry_snapshot() {
    use super::{
        native_inputs::{FighterValue, NativeInput, NativeObservation, NativeSnapshot},
        status_commands::{Command, Energy, EnergyOp, MotionFrame},
        status_model::Phase,
    };
    let mut graph: BehaviorGraph =
        serde_json::from_str(include_str!("../../tests/data/mario_status_own.json")).unwrap();
    let snapshot_speed = NativeInput::Fighter {
        value: FighterValue::SpeedX,
        fighter: "mario".into(),
    };
    graph.transitions[2].to = "cape".into();
    graph.transitions[2].guard = Predicate::SituationChanged;
    graph.transitions[2].actions = vec![
        Action::Status(Command::PlayMotion {
            motion: "special_air_s".into(),
            frame: MotionFrame::Inherit,
            rate: Expr::Const(ConstValue::Float(1.0)),
        }),
        Action::Status(Command::ConfigureEnergy {
            energy: Energy::Stop,
            op: EnergyOp::Speed {
                x: Expr::Const(ConstValue::Float(3.0)),
                y: Expr::Const(ConstValue::Float(0.0)),
            },
        }),
        Action::Status(Command::ConfigureEnergy {
            energy: Energy::Stop,
            op: EnergyOp::Speed {
                x: Expr::NativeFloat(snapshot_speed.clone()),
                y: Expr::Const(ConstValue::Float(0.0)),
            },
        }),
    ];
    let program = live_protocol::program_from_graph("mario", &graph).unwrap();
    let saved = serde_json::to_string(&graph).unwrap();
    assert_eq!(
        serde_json::from_str::<BehaviorGraph>(&saved).unwrap(),
        graph
    );
    let artifact = super::adapter::render(&std::collections::BTreeMap::from([(
        "mario".into(),
        graph.clone(),
    )]))
    .unwrap();
    assert!(artifact.source.contains("ConfigureEnergy"));
    assert!(artifact.source.contains("SituationChanged"));
    let mut state = program.initial_state();
    let mut snapshot =
        NativeSnapshot::from_observations(&[snapshot_speed], &[NativeObservation::Float(1.25)])
            .unwrap();
    program
        .step_status_phase(&mut state, "cape", Phase::Pre, &snapshot)
        .unwrap();
    program
        .step_status_phase(&mut state, "cape", Phase::MainEnter, &snapshot)
        .unwrap();
    snapshot.situation_changed = Some(true);
    let step = program
        .step_status_phase(&mut state, "cape", Phase::MainLoop, &snapshot)
        .unwrap();
    assert_eq!(step.engine_actions.len(), 3);
    assert!(matches!(
        &step.engine_actions[0],
        model::Action::Status(Command::PlayMotion {
            frame: MotionFrame::Inherit,
            ..
        })
    ));
    assert!(matches!(
        &step.engine_actions[2],
        model::Action::Status(Command::ConfigureEnergy {
            op: EnergyOp::Speed {
                x: model::Expr::Const(model::Value::Float(1.25)),
                ..
            },
            ..
        })
    ));
    snapshot.situation_changed = Some(false);
    assert!(program
        .step_status_phase(&mut state, "cape", Phase::MainLoop, &snapshot)
        .unwrap()
        .engine_actions
        .is_empty());
    snapshot.situation_changed = None;
    assert!(program
        .step_status_phase(&mut state, "cape", Phase::MainLoop, &snapshot)
        .is_err());
    let before = state.clone();
    let mut excessive = NativeSnapshot::from_observations(
        &[NativeInput::Fighter {
            value: FighterValue::SpeedX,
            fighter: "mario".into(),
        }],
        &[NativeObservation::Float(20.0)],
    )
    .unwrap();
    excessive.situation_changed = Some(true);
    assert!(program
        .step_status_phase(&mut state, "cape", Phase::MainLoop, &excessive)
        .is_err());
    assert_eq!(state, before);
}
