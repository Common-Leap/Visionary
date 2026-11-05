use super::*;
use crate::slight::gimmicks::{
    behavior as b, damage_dispatch as delivery, damage_feed, damage_feed_model as feed,
    native_inputs::NativeInput,
};

const SOURCE: u32 = 0;
const TARGET: u32 = 65536;
const ARTICLE: &str = "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE";
static EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(10_000);

fn fixture(amount: f32) -> (feed::Candidate, feed::Stamp, [delivery::Event; 2]) {
    use crate::slight::gimmicks::{
        contact_snapshot_model::Identity as ContactIdentity, damage_commit_model as commit,
        damage_delivery_model::Cursor,
    };
    crate::cloud(SOURCE, 0x1000, 0);
    crate::cloud(TARGET, 0x2000, 1);
    let actor = |object_id, accessor, entry| feed::Actor {
        identity: ContactIdentity {
            object_id,
            accessor,
            category: 0,
            kind: 62,
            entry,
            founder: None,
        },
        stock_generation: 1,
    };
    let source = actor(SOURCE, 0x1000, 0);
    let target = actor(TARGET, 0x2000, 1);
    let stamp = feed::Stamp {
        epoch: EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        frame: 20,
        revision: 5,
    };
    let sample = commit::Sample {
        epoch: stamp.epoch,
        frame: stamp.frame,
        index: 0,
        context_id: 0,
        caller_return_offset: Some(0x3febd8),
        before_bits: 999f32.to_bits(),
        pending_bits: amount.to_bits(),
        after_bits: 999f32.to_bits(),
        cap_bits: Some(999f32.to_bits()),
        suppressed: false,
        objects: [
            None,
            Some(
                crate::slight::gimmicks::contact_snapshot_model::Observation {
                    identity: target.identity,
                    percent_bits: Some(999f32.to_bits()),
                },
            ),
            None,
        ],
    };
    let contact = feed::Candidate {
        shield: false,
        weapon: None,
        stamp,
        contact_index: 0,
        update_index: 0,
        source,
        target,
        source_status: Some(0),
        target_status: Some(486),
        before_bits: sample.before_bits,
        after_bits: sample.after_bits,
        cap_bits: sample.cap_bits.unwrap(),
        amount: commit::confirmed_pending(&sample),
    };
    let next = feed::Stamp { frame: 21, ..stamp };
    damage_feed::set_stamp(Some(next));
    let events = Cursor::new().begin(contact, next).unwrap();
    (contact, next, events)
}

fn matches(boid: u32) -> bool {
    actor_matches(boid, "cloud")
}

#[test]
fn pending_damage_adapter_retains_each_receivers_historical_status_in_its_root_input() {
    let _serial = crate::TEST_LOCK.lock().unwrap();
    crate::reset();
    let (_, _, events) = fixture(3.0);
    let selector = NativeInput::FighterStatusIs {
        fighter: "cloud".into(),
        symbol: "FIGHTER_CLOUD_STATUS_KIND_SPECIAL_LW_CHARGE".into(),
    };
    for (event, expected) in [(events[0], false), (events[1], true)] {
        let input = crate::slight::gimmicks::damage_context::input(event).unwrap();
        let snapshot =
            crate::slight::gimmicks::native_inputs::NativeSnapshot::empty().with_damage(input);
        assert_eq!(snapshot.damage_status_is(&selector).unwrap(), expected);
        assert_eq!(snapshot.pending_damage().unwrap(), 3.0);
        assert!(snapshot
            .without_damage()
            .damage_status_is(&selector)
            .is_err());
    }
}
static ACTIONS: [b::Action; 4] = [
    b::Action::Add {
        var: "total",
        value: &b::Expr::PendingDamage,
    },
    b::Action::EmitEventValues {
        name: "nested",
        arguments: &[b::EventValueArgument {
            field: "amount",
            value: b::EventValue::Numeric(&b::Expr::PendingDamage),
        }],
    },
    b::Action::GenerateArticle { symbol: ARTICLE },
    b::Action::GenerateArticle { symbol: ARTICLE },
];
static NATIVE_GUARD: b::Predicate = b::Predicate::Compare {
    op: b::CmpOp::Gte,
    left: &b::Expr::NativeFloat(b::NativeSource("cloud_limit_gauge")),
    right: &b::Expr::Const(b::Value::Float(0.0)),
};
static TRANSITIONS: [b::Transition; 3] = [
    b::Transition {
        guard: &NATIVE_GUARD,
        actions: &ACTIONS,
        ..crate::damage_expression_tests::PROGRAM.transitions[0]
    },
    crate::damage_expression_tests::PROGRAM.transitions[1],
    b::Transition {
        guard: &NATIVE_GUARD,
        actions: &ACTIONS,
        ..crate::damage_expression_tests::PROGRAM.transitions[2]
    },
];
static PROGRAM: b::Program = b::Program {
    matches_fighter: matches,
    transitions: &TRANSITIONS,
    native_inputs: &[b::NativeSource("cloud_limit_gauge")],
    ..crate::damage_expression_tests::PROGRAM
};
fn static_handler(event: delivery::Event) {
    b::execute_damage_event(&PROGRAM, event).unwrap();
}
fn registrations() -> [b::DamageRegistration; 2] {
    [
        b::DamageRegistration {
            direction: delivery::Direction::Deal,
            handler: static_handler,
        },
        b::DamageRegistration {
            direction: delivery::Direction::Take,
            handler: static_handler,
        },
    ]
}

fn program(id: &str) -> model::LiveProgram {
    let mut program = crate::damage_expression_tests::owned();
    program.id = id.into();
    program.native_inputs = vec![NativeInput::CloudLimitGauge];
    for index in [0, 2] {
        program.transitions[index].guard = model::Predicate::Compare {
            op: model::CmpOp::Gte,
            left: model::Expr::NativeFloat(NativeInput::CloudLimitGauge),
            right: model::Expr::Const(model::Value::Float(0.0)),
        };
        program.transitions[index].actions.extend([
            model::Action::GenerateArticle {
                symbol: ARTICLE.into(),
            },
            model::Action::GenerateArticle {
                symbol: ARTICLE.into(),
            },
        ]);
    }
    assert!(program.validate().is_ok(), "{:?}", program.validate());
    program
}

// Adapter tests install validated models directly; protocol admission is
// exercised separately by the damage expression installation tests.
fn install(mode: u8) {
    let mut runtime = RUNTIME.lock();
    runtime.revision = 1;
    let coordinator = Arc::new(
        super::super::controller_runtime::ControllerCoordinator::new(BTreeMap::from([
            ("a".into(), program("damage-a")),
            ("z".into(), program("damage-z")),
        ]))
        .unwrap(),
    );
    if mode == 1 {
        runtime.controllers.insert("cloud".into(), coordinator);
    } else {
        runtime
            .compiled_controllers
            .insert("cloud".into(), coordinator);
    }
}

fn totals(mode: u8, boid: u32) -> Vec<f32> {
    let runtime = RUNTIME.lock();
    let present = if mode == 1 {
        runtime.controller_objects.contains_key(&boid)
    } else {
        runtime.compiled_controller_objects.contains_key(&boid)
    };
    drop(runtime);
    if !present {
        return vec![];
    }
    controllers::controller_wire_rows()
        .into_iter()
        .filter(|row| row.actor.boid == boid)
        .map(|row| match row.actor.vars["total"] {
            model::Value::Float(value) => value,
            _ => panic!(),
        })
        .collect()
}

fn ordinary_bundle(revision: u64, initial: i32) -> String {
    use crate::slight::gimmicks::controller_protocol as protocol;
    protocol::encode_request(&protocol::ControllerBundleRequest {
        version: protocol::REQUEST_VERSION,
        revision,
        disabled_fighters: vec![],
        charts: vec![protocol::ControllerProgram {
            chart_id: "main".into(),
            program: crate::ordinary_action_program(initial, vec![]),
        }],
    })
    .unwrap()
}

#[test]
fn pending_damage_configuration_replacement_fences_captured_contacts_before_new_owners() {
    let _serial = crate::TEST_LOCK.lock().unwrap();
    for v5 in [false, true] {
        crate::reset();
        let raw = |revision, initial| {
            if v5 {
                ordinary_bundle(revision, initial)
            } else {
                crate::request(
                    revision,
                    vec![crate::ordinary_action_program(initial, vec![])],
                    vec![],
                )
            }
        };
        let (_, _, old) = fixture(3.0);
        let changes = damage_feed::configuration_changes();
        // Initial revision zero is a real configuration change too.
        assert!(handle(&raw(0, 0)).accepted);
        assert_eq!(damage_feed::configuration_changes(), changes + 1);
        assert!(!damage_feed::event_is_current(old[0]));
        assert!(!damage_feed::event_is_current(old[1]));

        let (_, _, current) = fixture(3.0);
        assert!(handle(&raw(0, 0)).accepted);
        assert!(!handle(&raw(0, 1)).accepted);
        assert!(!handle("malformed").accepted);
        assert_eq!(damage_feed::configuration_changes(), changes + 1);
        assert!(damage_feed::event_is_current(current[0]));

        assert!(handle(&raw(1, 0)).accepted);
        assert_eq!(damage_feed::configuration_changes(), changes + 2);
        assert!(!damage_feed::event_is_current(current[0]));
        let (_, _, current) = fixture(3.0);
        assert!(!handle(&raw(0, 0)).accepted);
        assert!(damage_feed::event_is_current(current[0]));

        clear_overrides();
        assert_eq!(damage_feed::configuration_changes(), changes + 3);
        assert!(!damage_feed::event_is_current(current[0]));
        let (_, _, current) = fixture(3.0);
        clear_overrides();
        assert_eq!(damage_feed::configuration_changes(), changes + 3);
        assert!(damage_feed::event_is_current(current[0]));
    }
    crate::reset();
}

#[test]
fn pending_damage_compiled_baseline_install_and_removal_fence_contacts_but_retries_do_not() {
    let _serial = crate::TEST_LOCK.lock().unwrap();
    crate::reset();
    let (_, _, old) = fixture(3.0);
    let changes = damage_feed::configuration_changes();
    let raw = ordinary_bundle(0, 0);
    install_compiled_controllers(&raw).unwrap();
    assert_eq!(damage_feed::configuration_changes(), changes + 1);
    assert!(!damage_feed::event_is_current(old[0]));

    let (_, _, current) = fixture(3.0);
    install_compiled_controllers(&raw).unwrap();
    assert!(install_compiled_controllers(&ordinary_bundle(0, 1)).is_err());
    assert!(install_compiled_controllers(&ordinary_bundle(1, 0)).is_err());
    assert_eq!(damage_feed::configuration_changes(), changes + 1);
    assert!(damage_feed::event_is_current(current[0]));
    clear_compiled_baseline();
    assert_eq!(damage_feed::configuration_changes(), changes + 2);
    assert!(!damage_feed::event_is_current(current[0]));
    let (_, _, current) = fixture(3.0);
    clear_compiled_baseline();
    assert_eq!(damage_feed::configuration_changes(), changes + 2);
    assert!(damage_feed::event_is_current(current[0]));
    crate::reset();
}

#[test]
fn pending_damage_typed_registration_is_atomic_deduplicated_and_separate_from_legacy() {
    let _serial = crate::TEST_LOCK.lock().unwrap();
    crate::reset();
    let (contact, next, _) = fixture(3.0);
    let duplicate = [registrations()[0], registrations()[0]];
    assert!(b::register_with_damage(&PROGRAM, &[], &duplicate).is_err());
    assert_eq!(delivery::handler_count(delivery::Direction::Deal), 0);
    for _ in 0..2 {
        b::register_with_damage(&PROGRAM, &[], &registrations()).unwrap();
    }
    assert_eq!(delivery::handler_count(delivery::Direction::Deal), 1);
    assert_eq!(delivery::handler_count(delivery::Direction::Take), 1);
    dispatcher::fire(dispatcher::GimmickHook::DealDamage, SOURCE);
    assert_eq!(
        b::read(&PROGRAM, SOURCE, "total").unwrap(),
        b::Value::Float(0.0)
    );
    assert_eq!(
        delivery::deliver(contact, next, || true),
        delivery::Outcome::Delivered { handlers: 2 }
    );
    for actor in [SOURCE, TARGET] {
        assert_eq!(
            b::read(&PROGRAM, actor, "total").unwrap(),
            b::Value::Float(3.0)
        );
        assert_eq!(
            b::read(&PROGRAM, actor, "copied").unwrap(),
            b::Value::Float(3.0)
        );
    }
    assert!(matches!(
        delivery::deliver(contact, next, || true),
        delivery::Outcome::Rejected(_)
    ));
    assert_eq!(crate::slight::gimmicks::operations::take_actions().len(), 4);
    crate::reset();
}

#[test]
fn pending_damage_static_contact_guard_rejects_stale_and_mid_action_other_participant_changes() {
    let _serial = crate::TEST_LOCK.lock().unwrap();
    for change in [0, 1, 2] {
        crate::reset();
        let (_, next, events) = fixture(3.0);
        match change {
            0 => damage_feed::set_stamp(Some(feed::Stamp {
                frame: next.frame + 1,
                ..next
            })),
            1 => crate::cloud(TARGET, 0x3000, 1),
            _ => crate::slight::gimmicks::operations::hook_next_action_change_identity(
                TARGET, 0x3000, 62, 1,
            ),
        }
        assert!(b::execute_damage_event(&PROGRAM, events[0]).is_err());
        assert_eq!(
            b::read(&PROGRAM, SOURCE, "total").unwrap(),
            b::Value::Float(0.0)
        );
        assert_eq!(
            crate::slight::gimmicks::operations::take_actions().len(),
            usize::from(change == 2)
        );
    }
    crate::reset();
}

#[test]
fn pending_damage_owned_routes_select_both_directions_and_compiled_or_live_charts_once() {
    let _serial = crate::TEST_LOCK.lock().unwrap();
    for mode in 1..3 {
        crate::reset();
        let (contact, next, _) = fixture(3.0);
        install(mode);
        register_damage_routes();
        register_damage_routes();
        assert_eq!(delivery::handler_count(delivery::Direction::Deal), 1);
        dispatcher::fire(dispatcher::GimmickHook::DealDamage, SOURCE);
        assert!(totals(mode, SOURCE).is_empty());
        assert_eq!(
            delivery::deliver(contact, next, || true),
            delivery::Outcome::Delivered { handlers: 2 }
        );
        let charts = if mode == 0 { 1 } else { 2 };
        for actor in [SOURCE, TARGET] {
            assert_eq!(totals(mode, actor), vec![3.0; charts]);
        }
        assert!(matches!(
            delivery::deliver(contact, next, || true),
            delivery::Outcome::Rejected(_)
        ));
        assert_eq!(
            crate::slight::gimmicks::operations::take_actions().len(),
            charts * 4
        );
    }
    crate::reset();
}

#[test]
fn pending_damage_owned_other_actor_change_stops_remaining_actions_and_defender_delivery() {
    let _serial = crate::TEST_LOCK.lock().unwrap();
    for mode in 1..3 {
        crate::reset();
        let (contact, next, events) = fixture(3.0);
        install(mode);
        register_damage_routes();
        crate::slight::gimmicks::operations::hook_next_action_change_identity(
            TARGET, 0x3000, 62, 1,
        );
        let outcome = delivery::deliver(contact, next, || damage_feed::event_is_current(events[0]));
        assert!(matches!(
            outcome,
            delivery::Outcome::Invalidated { handlers: 1 }
        ));
        let charts = if mode == 0 { 1 } else { 2 };
        // Ordinary owned dispatch consumes authored state before native effects.
        assert_eq!(totals(mode, SOURCE), vec![3.0; charts]);
        assert!(totals(mode, TARGET).is_empty());
        assert_eq!(crate::slight::gimmicks::operations::take_actions().len(), 1);
    }
    crate::reset();
}

#[test]
fn pending_damage_sampling_configuration_change_cancels_static_and_owned_commits() {
    let _serial = crate::TEST_LOCK.lock().unwrap();
    for mode in 1..4 {
        crate::reset();
        let (_, _, events) = fixture(3.0);
        if mode < 3 {
            install(mode);
        }
        let empty = crate::request(2, vec![], vec!["cloud".into()]);
        crate::slight::gimmicks::native_values::replace_snapshot_after_next_sample(empty);
        if mode < 3 {
            dispatch_damage(events[0]);
            assert!(totals(mode, SOURCE).is_empty());
        } else {
            assert!(b::execute_damage_event(&PROGRAM, events[0]).is_err());
            assert_eq!(
                b::read(&PROGRAM, SOURCE, "total").unwrap(),
                b::Value::Float(0.0)
            );
        }
        assert!(crate::slight::gimmicks::operations::take_actions().is_empty());
    }
    crate::reset();
}

#[test]
fn pending_damage_other_participant_or_frame_change_during_sampling_cancels_every_adapter() {
    let _serial = crate::TEST_LOCK.lock().unwrap();
    fn change_target() {
        crate::cloud(TARGET, 0x3000, 1);
    }
    fn invalidate_frame() {
        damage_feed::set_stamp(None);
    }
    for callback in [change_target as fn(), invalidate_frame as fn()] {
        for mode in 1..4 {
            crate::reset();
            let (_, _, events) = fixture(3.0);
            if mode < 3 {
                install(mode);
            }
            crate::slight::gimmicks::native_values::after_next_sample(callback);
            if mode < 3 {
                dispatch_damage(events[0]);
                assert!(totals(mode, SOURCE).is_empty());
            } else {
                assert!(b::execute_damage_event(&PROGRAM, events[0]).is_err());
                assert_eq!(
                    b::read(&PROGRAM, SOURCE, "total").unwrap(),
                    b::Value::Float(0.0)
                );
            }
            assert!(crate::slight::gimmicks::operations::take_actions().is_empty());
        }
    }
    crate::reset();
}

#[test]
fn captured_weapon_payload_reaches_owned_predicates_without_a_surviving_weapon() {
    let _serial = crate::TEST_LOCK.lock().unwrap();
    for projectile in [false, true] {
        crate::reset();
        let (mut contact, next, _) = fixture(3.0);
        if projectile {
            contact.weapon = Some(super::super::contact_snapshot_model::Identity {
                object_id: 0x10000001,
                accessor: 0x3000,
                category: 1,
                kind: 4,
                entry: -1,
                founder: Some(SOURCE),
            });
        }
        let events = super::super::damage_delivery_model::Cursor::new()
            .begin(contact, next)
            .unwrap();
        let mut program = crate::damage_expression_tests::owned();
        for index in [0, 2] {
            program.transitions[index].guard = model::Predicate::DamageIsProjectile;
        }
        assert!(program.validate().is_ok());
        for event in events {
            assert!(damage_feed::event_is_current(event));
            let input = super::super::damage_context::input(event).unwrap();
            let mut state = program.initial_state();
            program
                .step_damage_with_snapshot(
                    &mut state,
                    input,
                    &super::super::native_inputs::NativeSnapshot::empty(),
                )
                .unwrap();
            assert_eq!(
                state.values["total"],
                model::Value::Float(if projectile { 3.0 } else { 0.0 })
            );
        }
    }
    crate::reset();
}
