//! Closed resource actions: authoring, captured values and actual source fixture.
use super::resource_commands::{Command, Op, Resource};
use super::{
    behavior::{
        self, Action, BehaviorGraph, CmpOp, ConstValue, Expr, Predicate, StateDef, Transition,
        VarDef, VarType,
    },
    codegen::{EmissionPlan, HookBinding},
    hooks::HookName,
};
use std::collections::BTreeMap;

fn limit<E>(op: Op, amount: E) -> Command<E> {
    Command {
        op,
        resource: Resource::named("cloudLimit").unwrap(),
        amount,
    }
}

pub(super) fn graph() -> BehaviorGraph {
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
        vars: vec![VarDef {
            keep_across_form_switch: false,
            name: "amount".into(),
            kind: VarType::Float,
            initial: ConstValue::Float(2.0),
        }],
        transitions: vec![Transition {
            from: "idle".into(),
            to: "done".into(),
            on_frame: true,
            guard: Predicate::Compare {
                op: CmpOp::Eq,
                left: Expr::Const(ConstValue::Int(1)),
                right: Expr::Const(ConstValue::Int(1)),
            },
            actions: vec![
                Action::Resource(limit(Op::Add, Expr::Var("amount".into()))),
                Action::Set {
                    var: "amount".into(),
                    value: Expr::Const(ConstValue::Float(9.0)),
                },
                Action::Resource(limit(Op::Set, Expr::Const(ConstValue::Float(0.0)))),
                Action::Resource(limit(Op::Add, Expr::Var("amount".into()))),
            ],
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
fn resource_owner_type_persistence_and_value_references_are_checked() {
    let graph = graph();
    assert!(behavior::validate_for_fighter("cloud", &graph).is_empty());
    assert!(!behavior::validate_for_fighter("mario", &graph).is_empty());
    let saved = serde_json::to_string(&graph).unwrap();
    let loaded: BehaviorGraph = serde_json::from_str(&saved).unwrap();
    assert_eq!(loaded, graph);
    let changed = super::graph_edit::rename_variable(&graph, "cloud", "amount", "gain").unwrap();
    assert!(
        matches!(&changed.transitions[0].actions[0],Action::Resource(Command{amount:Expr::Var(name),..})if name=="gain")
    );
    assert!(
        matches!(&changed.transitions[0].actions[3],Action::Resource(Command{amount:Expr::Var(name),..})if name=="gain")
    );
    let mut wrong = graph.clone();
    wrong.transitions[0].actions[0] =
        Action::Resource(limit(Op::Add, Expr::Const(ConstValue::Int(2))));
    assert!(!behavior::validate_for_fighter("cloud", &wrong).is_empty());
    wrong.transitions[0].actions[0] =
        Action::Resource(limit(Op::Set, Expr::Const(ConstValue::Float(101.0))));
    assert!(!behavior::validate_for_fighter("cloud", &wrong).is_empty());
    wrong.transitions[0].actions[0] = Action::Resource(limit(Op::Add, Expr::PendingDamage));
    assert!(super::adapter::render(&BTreeMap::from([("cloud".into(), wrong)])).is_err());
    // Saves from before the table: Cloud's two Limit actions and constant resource amounts.
    let legacy: Vec<Action> = serde_json::from_value(serde_json::json!([
        {"AdjustCloudLimit": {"amount": {"Var": "amount"}}}, "ClearCloudLimit", {"ClearCloudLimit": null},
        {"Resource": {"op": "add", "resource": "inklingInk", "amount": 0.5}},
    ])).unwrap();
    assert_eq!(
        legacy[..3],
        graph.transitions[0].actions[..1]
            .iter()
            .chain(&graph.transitions[0].actions[2..3])
            .chain(&graph.transitions[0].actions[2..3])
            .cloned()
            .collect::<Vec<_>>()[..]
    );
    assert_eq!(
        legacy[3],
        Action::Resource(Command {
            op: Op::Add,
            resource: Resource::named("inklingInk").unwrap(),
            amount: Expr::Const(ConstValue::Float(0.5))
        })
    );
    let saved = serde_json::to_value(&legacy).unwrap();
    assert_eq!(
        serde_json::from_value::<Vec<Action>>(saved).unwrap(),
        legacy
    );
}

#[test]
fn owned_resource_effects_capture_each_amount_at_its_action_position() {
    use super::live_model as m;
    let p = super::live_protocol::offline_program_from_graph("cloud", &graph()).unwrap();
    let mut state = p.initial_state();
    let step = p.step_frame(&mut state).unwrap();
    assert_eq!(state.values["amount"], m::Value::Float(9.0));
    assert_eq!(
        step.engine_actions,
        vec![
            m::Action::Resource(limit(Op::Add, m::Expr::Const(m::Value::Float(2.0)))),
            m::Action::Resource(limit(Op::Set, m::Expr::Const(m::Value::Float(0.0)))),
            m::Action::Resource(limit(Op::Add, m::Expr::Const(m::Value::Float(9.0)))),
        ]
    );
    assert!(
        matches!(&step.fired[0].actions[0],m::Action::Resource(Command{amount:m::Expr::Var(name),..})if name=="amount")
    );
    assert!(p
        .required_native_features()
        .contains(super::native_inputs::CLOUD_LIMIT_ACTIONS_FEATURE));
    let mut foreign = p.clone();
    foreign.fighter = "mario".into();
    assert!(foreign.validate().is_err());
}

mod table {
    use super::super::native_inputs::NativeKind;
    use super::super::registry::{self, IdentifierKind};
    use super::super::resource_commands::*;

    fn command(key: &str, op: Op, amount: f32) -> Command {
        Command {
            op,
            resource: Resource::named(key).unwrap(),
            amount,
        }
    }

    #[test]
    fn every_row_is_a_registered_work_field_of_its_owner_with_a_unique_key() {
        let mut keys = std::collections::HashSet::new();
        for entry in RESOURCES {
            assert!(keys.insert(entry.key), "duplicate key {}", entry.key);
            if entry.writer == Writer::PartnerDamage {
                continue;
            }
            let kind = if entry.kind == NativeKind::Int {
                IdentifierKind::WorkInt
            } else {
                IdentifierKind::WorkFloat
            };
            registry::resolve(entry.fighter, entry.symbol, kind)
                .unwrap_or_else(|e| panic!("{}: {e}", entry.key));
            assert_eq!(entry.input().fighter(), Some(entry.fighter));
            assert!(entry.max > 0.0);
            if let Writer::PickelMaterial(kind) = entry.writer {
                assert_eq!(
                    entry.symbol.rsplit("MATERIAL_NUM_").next(),
                    kind.rsplit("MATERIAL_KIND_").next()
                );
            }
        }
    }

    #[test]
    fn add_and_set_clamp_to_the_row_and_saved_keys_round_trip() {
        assert_eq!(
            command("inklingInk", Op::Add, -100.0).target(12.5).unwrap(),
            0.0
        );
        assert_eq!(
            command("inklingInk", Op::Add, 0.5).target(12.5).unwrap(),
            13.0
        );
        assert_eq!(
            command("cloudLimit", Op::Set, 100.0).target(30.0).unwrap(),
            100.0
        );
        assert_eq!(
            command("banjoFeathers", Op::Add, 3.0).target(4.0).unwrap(),
            5.0
        );
        assert!(command("banjoFeathers", Op::Add, 0.5).target(1.0).is_err());
        assert!(command("banjoFeathers", Op::Set, 6.0).target(1.0).is_err());
        assert!(command("inklingInk", Op::Add, 1.0)
            .target(f32::NAN)
            .is_err());
        let saved = serde_json::json!({"op": "add", "resource": "inklingInk", "amount": 0.5});
        let refill: Command = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(serde_json::to_value(refill).unwrap(), saved);
        assert!(refill.validate("inkling").is_ok());
        assert!(refill.validate("kirby").is_err());
        assert!(serde_json::from_value::<Command>(
            serde_json::json!({"op": "add", "resource": "kirbyStars", "amount": 1.0})
        )
        .is_err());
    }
}
