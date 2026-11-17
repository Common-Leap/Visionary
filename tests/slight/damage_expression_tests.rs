use super::{reset, TEST_LOCK};
use crate::slight::gimmicks::native_inputs::{DamageDirection, DamageInput, NativeSnapshot};
use crate::slight::gimmicks::{
    behavior as b, dispatcher::GimmickHook as H, live_behavior, live_model as m,
};

const BOID: u32 = 0xF0AA_0009;
fn matches(boid: u32) -> bool {
    boid == BOID
}
fn handler(_: u32) {}
fn typed_handler(_: crate::slight::gimmicks::damage_dispatch::Event) {}
static CAPTURE_STATUS: b::Predicate =
    b::Predicate::DamageStatusIs("cloud", "FIGHTER_CLOUD_STATUS_KIND_SPECIAL_LW_CHARGE");
static CAPTURE_TRANSITIONS: [b::Transition; 3] = [
    b::Transition {
        guard: &CAPTURE_STATUS,
        ..TRANSITIONS[0]
    },
    TRANSITIONS[1],
    b::Transition {
        guard: &CAPTURE_STATUS,
        ..TRANSITIONS[2]
    },
];
static CAPTURE_PROGRAM: b::Program = b::Program {
    id: "contact_status_parity",
    transitions: &CAPTURE_TRANSITIONS,
    ..PROGRAM
};

#[test]
fn contact_status_static_registration_and_owned_execution_agree_without_native_sampling_sources() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    let regs = [
        b::DamageRegistration {
            direction: crate::slight::gimmicks::damage_dispatch::Direction::Deal,
            handler: typed_handler,
        },
        b::DamageRegistration {
            direction: crate::slight::gimmicks::damage_dispatch::Direction::Take,
            handler: typed_handler,
        },
    ];
    b::register_with_damage(&CAPTURE_PROGRAM, &[], &regs).unwrap();
    let mut owned = owned();
    let selector = crate::slight::gimmicks::native_inputs::NativeInput::FighterStatusIs {
        fighter: "cloud".into(),
        symbol: "FIGHTER_CLOUD_STATUS_KIND_SPECIAL_LW_CHARGE".into(),
    };
    for index in [0, 2] {
        owned.transitions[index].guard = m::Predicate::DamageStatusIs(selector.clone());
    }
    assert!(owned.validate().is_ok());
    assert!(owned.native_inputs.is_empty() && CAPTURE_PROGRAM.native_inputs.is_empty());
    let mut state = owned.initial_state();
    for (direction, status, expected) in [
        (DamageDirection::Deal, 0, 0.0),
        (DamageDirection::Take, 486, 3.0),
        (DamageDirection::Take, 73, 3.0),
        (DamageDirection::Deal, 486, 6.0),
    ] {
        let input = DamageInput::new(direction, 3.0)
            .unwrap()
            .with_receiver_status(62, Some(status))
            .unwrap();
        b::execute_damage_input(&CAPTURE_PROGRAM, BOID, input).unwrap();
        owned
            .step_damage_with_snapshot(&mut state, input, &NativeSnapshot::empty())
            .unwrap();
        assert_eq!(state.values["total"], m::Value::Float(expected));
        assert_eq!(
            b::read(&CAPTURE_PROGRAM, BOID, "total").unwrap(),
            b::Value::Float(expected)
        );
    }
    let missing = DamageInput::new(DamageDirection::Take, 3.0).unwrap();
    assert!(b::execute_damage_input(&CAPTURE_PROGRAM, BOID, missing).is_err());
    assert_eq!(
        b::read(&CAPTURE_PROGRAM, BOID, "total").unwrap(),
        b::Value::Float(6.0)
    );
    static BAD_TRANSITIONS: [b::Transition; 3] = [
        CAPTURE_TRANSITIONS[0],
        b::Transition {
            guard: &CAPTURE_STATUS,
            ..CAPTURE_TRANSITIONS[1]
        },
        CAPTURE_TRANSITIONS[2],
    ];
    static BAD: b::Program = b::Program {
        id: "bad_contact_status_scope",
        transitions: &BAD_TRANSITIONS,
        ..CAPTURE_PROGRAM
    };
    assert!(b::register_with_damage(&BAD, &[], &regs).is_err());
    reset();
}
static TRUE: b::Predicate = b::Predicate::Compare {
    op: b::CmpOp::Eq,
    left: &b::Expr::Const(b::Value::Int(0)),
    right: &b::Expr::Const(b::Value::Int(0)),
};
static PENDING: b::Expr = b::Expr::PendingDamage;
static ROOT: [b::Action; 2] = [
    b::Action::Add {
        var: "total",
        value: &PENDING,
    },
    b::Action::EmitEventValues {
        name: "nested",
        arguments: &[b::EventValueArgument {
            field: "amount",
            value: b::EventValue::Numeric(&PENDING),
        }],
    },
];
static RECEIVER: [b::Action; 1] = [b::Action::Set {
    var: "copied",
    value: &b::Expr::EventField {
        event: "nested",
        field: "amount",
    },
}];
static TRANSITIONS: [b::Transition; 3] = [
    b::Transition {
        from: 0,
        to: 0,
        guard: &TRUE,
        actions: &ROOT,
    },
    b::Transition {
        from: 0,
        to: 0,
        guard: &TRUE,
        actions: &RECEIVER,
    },
    b::Transition {
        from: 0,
        to: 0,
        guard: &TRUE,
        actions: &ROOT,
    },
];
pub(crate) static PROGRAM: b::Program = b::Program {
    id: "pending_damage",
    fighter: "cloud",
    matches_fighter: matches,
    initial_state: 0,
    vars: &[
        b::VarDef {
            name: "total",
            kind: b::VarKind::Float,
            initial: b::Value::Float(0.0),
        },
        b::VarDef {
            name: "copied",
            kind: b::VarKind::Float,
            initial: b::Value::Float(0.0),
        },
    ],
    native_inputs: &[],
    transitions: &TRANSITIONS,
    bindings: &[
        b::Binding {
            hook: H::DealDamage,
            article: None,
            transitions: &[0],
        },
        b::Binding {
            hook: H::TakeDamage,
            article: None,
            transitions: &[2],
        },
    ],
    event_bindings: &[b::EventBinding {
        name: "nested",
        transitions: &[1],
        fields: &[b::EventField {
            name: "amount",
            kind: b::VarKind::Float,
        }],
    }],
};

pub(crate) fn owned() -> m::LiveProgram {
    let mut program: m::LiveProgram = serde_json::from_value(serde_json::json!({
        "version":4,"id":"pending_damage","fighter":"cloud","initial":"idle","states":["idle"],
        "vars":[{"name":"total","kind":"float","initial":{"Float":0.0}},{"name":"copied","kind":"float","initial":{"Float":0.0}}],
        "transitions":[
            {"from":"idle","to":"idle","on_frame":true,"guard":{"Compare":{"op":"eq","left":{"Const":{"Int":0}},"right":{"Const":{"Int":0}}}},"actions":[
                {"Add":{"var":"total","value":"PendingDamage"}},
                {"EmitEventValues":{"name":"nested","arguments":[{"field":"amount","value":{"Numeric":"PendingDamage"}}]}}
            ]},
            {"from":"idle","to":"idle","on_frame":true,"guard":{"Compare":{"op":"eq","left":{"Const":{"Int":0}},"right":{"Const":{"Int":0}}}},"actions":[
                {"Set":{"var":"copied","value":{"EventField":{"event":"nested","field":"amount"}}}}
            ]}
        ],"bindings":[{"hook":"onDealDamage","transitions":[0]},{"hook":"onTakeDamage","transitions":[0]}],
        "event_bindings":[{"name":"nested","transitions":[1],"fields":[{"name":"amount","kind":"float"}]}],"status_triggers":[],"resource_triggers":[]
    })).unwrap();
    program.transitions.push(program.transitions[0].clone());
    program.bindings[1].transitions = vec![2];
    program
}

#[test]
fn pending_damage_static_and_owned_interpreters_agree_and_only_forward_explicit_scalars() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    b::register_with_damage(
        &PROGRAM,
        &[],
        &[
            b::DamageRegistration {
                direction: crate::slight::gimmicks::damage_dispatch::Direction::Deal,
                handler: typed_handler,
            },
            b::DamageRegistration {
                direction: crate::slight::gimmicks::damage_dispatch::Direction::Take,
                handler: typed_handler,
            },
        ],
    )
    .unwrap();
    let program = owned();
    assert!(program.validate().is_ok(), "{:?}", program.validate());
    let mut state = program.initial_state();
    for (direction, amount) in [
        (DamageDirection::Deal, 3.0),
        (DamageDirection::Take, 1.44),
        (DamageDirection::Deal, 2.4),
    ] {
        let input = DamageInput::new(direction, amount).unwrap();
        b::execute_damage_input(&PROGRAM, BOID, input).unwrap();
        program
            .step_damage_with_snapshot(&mut state, input, &NativeSnapshot::empty())
            .unwrap();
        for name in ["total", "copied"] {
            let b::Value::Float(actual) = b::read(&PROGRAM, BOID, name).unwrap() else {
                panic!()
            };
            assert_eq!(state.values[name], m::Value::Float(actual));
        }
    }
    let before = b::read(&PROGRAM, BOID, "total").unwrap();
    assert!(b::execute(&PROGRAM, H::DealDamage, BOID).is_err());
    assert!(b::execute(&PROGRAM, H::TakeDamage, BOID).is_err());
    assert_eq!(b::read(&PROGRAM, BOID, "total").unwrap(), before);
    reset();
}

#[test]
fn pending_damage_static_validation_checks_hidden_and_mixed_scopes_and_runtime_strips_nested_input()
{
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    static BAD_RECEIVER: [b::Action; 1] = [b::Action::Set {
        var: "copied",
        value: &PENDING,
    }];
    static BAD_TRANSITIONS: [b::Transition; 3] = [
        b::Transition {
            from: 0,
            to: 0,
            guard: &TRUE,
            actions: &ROOT,
        },
        b::Transition {
            from: 0,
            to: 0,
            guard: &TRUE,
            actions: &BAD_RECEIVER,
        },
        b::Transition {
            from: 0,
            to: 0,
            guard: &TRUE,
            actions: &ROOT,
        },
    ];
    static BAD: b::Program = b::Program {
        transitions: &BAD_TRANSITIONS,
        ..PROGRAM
    };
    assert!(b::register_with_damage(
        &BAD,
        &[],
        &[
            b::DamageRegistration {
                direction: crate::slight::gimmicks::damage_dispatch::Direction::Deal,
                handler: typed_handler
            },
            b::DamageRegistration {
                direction: crate::slight::gimmicks::damage_dispatch::Direction::Take,
                handler: typed_handler
            },
        ]
    )
    .is_err());
    let before = b::read(&BAD, BOID, "total").unwrap();
    assert!(b::execute_damage_input(
        &BAD,
        BOID,
        DamageInput::new(DamageDirection::Deal, 3.0).unwrap()
    )
    .is_err());
    assert_eq!(b::read(&BAD, BOID, "total").unwrap(), before);
    assert!(b::execute_named_with_payload(&BAD, BOID, "nested", &[b::Value::Float(1.0)]).is_err());
    static MIXED: b::Program = b::Program {
        bindings: &[
            b::Binding {
                hook: H::DealDamage,
                article: None,
                transitions: &[0],
            },
            b::Binding {
                hook: H::ChargeTick,
                article: None,
                transitions: &[0],
            },
        ],
        ..PROGRAM
    };
    assert!(b::register_with_damage(
        &MIXED,
        &[b::Registration {
            hook: H::ChargeTick,
            handler,
            status: None,
            transitions: None
        }],
        &[b::DamageRegistration {
            direction: crate::slight::gimmicks::damage_dispatch::Direction::Deal,
            handler: typed_handler
        }]
    )
    .is_err());
    reset();
}

#[test]
fn pending_damage_live_v4_v5_and_compiled_controllers_accept_only_scoped_payloads() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    let program = owned();
    assert!(program.validate().is_ok());
    let raw = super::request(1, vec![program.clone()], vec![]);
    let ack = live_behavior::handle(&raw);
    assert!(ack.accepted, "{}", ack.message);
    let mut bundle = crate::slight::gimmicks::controller_protocol::ControllerBundleRequest {
        version: 5,
        revision: 1,
        disabled_fighters: vec![],
        charts: vec![
            crate::slight::gimmicks::controller_protocol::ControllerProgram {
                chart_id: "main".into(),
                program,
            },
        ],
    };
    let raw = crate::slight::gimmicks::controller_protocol::encode_request(&bundle).unwrap();
    let ack = live_behavior::handle(&raw);
    assert!(ack.accepted, "{}", ack.message);
    bundle.revision = 0;
    let raw = crate::slight::gimmicks::controller_protocol::encode_request(&bundle).unwrap();
    live_behavior::test_install_compiled_baseline(&raw).unwrap();
    // Removing root ownership must still reject the payload at installation.
    bundle.charts[0].program.bindings[0].hook = m::HookName::OnChargeTick;
    assert!(crate::slight::gimmicks::controller_protocol::encode_request(&bundle).is_err());
    let raw = serde_json::to_string(&serde_json::json!({"gimmick_behavior": bundle})).unwrap();
    assert!(live_behavior::test_install_compiled_baseline(&raw).is_err());
    reset();
}

#[test]
fn blocked_power_static_and_owned_interpreters_agree_for_every_shield_contact() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    static SHIELD: b::Program = b::Program {
        id: "blocked_power",
        bindings: &[
            b::Binding {
                hook: H::AttackShielded,
                article: None,
                transitions: &[0],
            },
            b::Binding {
                hook: H::ShieldHit,
                article: None,
                transitions: &[2],
            },
        ],
        ..PROGRAM
    };
    b::register_with_damage(
        &SHIELD,
        &[],
        &[
            b::DamageRegistration {
                direction: crate::slight::gimmicks::damage_dispatch::Direction::AttackShielded,
                handler: typed_handler,
            },
            b::DamageRegistration {
                direction: crate::slight::gimmicks::damage_dispatch::Direction::ShieldHit,
                handler: typed_handler,
            },
        ],
    )
    .unwrap();
    let mut program = owned();
    program.id = "blocked_power".into();
    program.bindings[0].hook = m::HookName::OnAttackShielded;
    program.bindings[1].hook = m::HookName::OnShieldHit;
    program.validate().unwrap();
    let mut state = program.initial_state();
    for direction in [
        DamageDirection::AttackShielded,
        DamageDirection::ShieldHit,
        DamageDirection::ShieldHit,
    ] {
        for power in [0.0, 4.5] {
            let input = DamageInput::new(direction, power).unwrap();
            b::execute_damage_input(&SHIELD, BOID, input).unwrap();
            program
                .step_damage_with_snapshot(&mut state, input, &NativeSnapshot::empty())
                .unwrap();
            for name in ["total", "copied"] {
                let b::Value::Float(actual) = b::read(&SHIELD, BOID, name).unwrap() else {
                    panic!()
                };
                assert_eq!(state.values[name], m::Value::Float(actual));
            }
        }
        let before = b::read(&SHIELD, BOID, "total").unwrap();
        assert!(
            b::execute_damage_input(&SHIELD, BOID, DamageInput::shield(direction).unwrap())
                .is_err()
        );
        assert_eq!(b::read(&SHIELD, BOID, "total").unwrap(), before);
    }
    reset();
}
