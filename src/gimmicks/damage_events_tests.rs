//! Production damage roots exercise the same models compiled into the plugin.
#![allow(dead_code)]
#[path = "../../plugins/slight_replica/src/slight/gimmicks/contact_snapshot_model.rs"]
mod contact_snapshot_model;
#[path = "../../plugins/slight_replica/src/slight/gimmicks/damage_capture_model.rs"]
mod damage_capture_model;
#[path = "../../plugins/slight_replica/src/slight/gimmicks/damage_commit_model.rs"]
mod damage_commit_model;
#[path = "../../plugins/slight_replica/src/slight/gimmicks/damage_delivery_model.rs"]
mod damage_delivery_model;
#[path = "../../plugins/slight_replica/src/slight/gimmicks/damage_feed_model.rs"]
mod damage_feed_model;
use super::{behavior::*, hooks::HookName, live_model as m, native_inputs::*};
use damage_feed_model as feed;

fn graph(hook: HookName) -> BehaviorGraph {
    let mut graph = super::resource_actions_tests::graph();
    graph.transitions[0].guard = Predicate::DamageIsProjectile;
    graph.transitions[0].actions = vec![Action::Set {
        var: "amount".into(),
        value: Expr::PendingDamage,
    }];
    graph.emission_plan.bindings[0].hook = hook;
    graph
}

#[test]
fn damage_payloads_are_receiver_scoped_and_static_live_identical() {
    for (hook, direction) in [
        (HookName::OnDealDamage, DamageDirection::Deal),
        (HookName::OnTakeDamage, DamageDirection::Take),
    ] {
        let graph = graph(hook);
        assert!(validate_for_fighter("cloud", &graph).is_empty());
        let program = super::live_protocol::program_from_graph("cloud", &graph).unwrap();
        assert!(!program.uses_controller_input());
        let mut mixed = graph.clone();
        mixed
            .emission_plan
            .bindings
            .push(super::codegen::HookBinding {
                hook: HookName::OnChargeTick,
                article: None,
                transitions: vec![0],
            });
        assert!(!validate(&mixed).is_empty());
        mixed.transitions[0].guard = Predicate::Not(Box::new(Predicate::DamageIsProjectile));
        mixed.transitions[0].actions.clear();
        assert!(!validate(&mixed).is_empty());
        for projectile in [true, false] {
            let mut state = program.initial_state();
            let input = DamageInput::new(direction, 4.5)
                .unwrap()
                .with_projectile(projectile);
            let result = program
                .step_damage_with_snapshot(&mut state, input, &NativeSnapshot::empty())
                .unwrap();
            assert_eq!(result.fired.len(), usize::from(projectile));
            assert_eq!(
                state.values["amount"],
                m::Value::Float(if projectile { 4.5 } else { 2.0 })
            );
        }
        assert!(program
            .step(
                &mut program.initial_state(),
                match direction {
                    DamageDirection::Deal => m::HookName::OnDealDamage,
                    DamageDirection::Take => m::HookName::OnTakeDamage,
                    DamageDirection::AttackShielded => m::HookName::OnAttackShielded,
                    DamageDirection::ShieldHit => m::HookName::OnShieldHit,
                    DamageDirection::Incoming => m::HookName::OnIncomingHit,
                    DamageDirection::Reflect => m::HookName::OnReflect,
                    DamageDirection::Absorb => m::HookName::OnAbsorb,
                }
            )
            .is_err());
        let rendered = super::adapter::render(&std::collections::BTreeMap::from([(
            "cloud".into(),
            graph.clone(),
        )]))
        .unwrap();
        let encoded = rendered
            .source
            .lines()
            .find_map(|line| line.strip_prefix("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = "))
            .unwrap();
        let encoded: String = serde_json::from_str(encoded.strip_suffix(';').unwrap()).unwrap();
        let bundle = super::controller_protocol::decode_request(&encoded).unwrap();
        assert_eq!(bundle.charts[0].program, program);
        let json = serde_json::to_value(&graph).unwrap();
        assert_eq!(json["transitions"][0]["guard"], "DamageIsProjectile");
        assert_eq!(
            json["transitions"][0]["actions"][0]["Set"]["value"],
            "PendingDamage"
        );
        assert_eq!(
            serde_json::from_value::<BehaviorGraph>(json).unwrap(),
            graph
        );
        for value in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(DamageInput::new(direction, value).is_err());
        }
        let mut outside = graph.clone();
        outside.emission_plan.bindings[0].hook = HookName::OnChargeTick;
        assert!(validate(&outside)
            .iter()
            .any(|e| e.reason.contains("projectile damage classification")));
        assert!(validate(&outside)
            .iter()
            .any(|e| e.reason.contains("pending damage")));
        let mut wire = program.clone();
        wire.bindings[0].hook = m::HookName::OnChargeTick;
        assert!(wire.validate().is_err());
        outside.transitions[0].actions.clear();
        assert!(!validate(&outside).is_empty());
        outside.transitions[0].guard = Predicate::Compare {
            op: CmpOp::Eq,
            left: Expr::Const(ConstValue::Int(1)),
            right: Expr::Const(ConstValue::Int(1)),
        };
        assert!(validate(&outside).is_empty());
    }
}

fn actor(id: u32) -> feed::Actor {
    feed::Actor {
        identity: contact_snapshot_model::Identity {
            object_id: id,
            accessor: 0x1000 + u64::from(id) * 8,
            category: 0,
            kind: 0,
            entry: id as i32,
            founder: None,
        },
        stock_generation: 1,
    }
}
fn observation(actor: feed::Actor, percent: f32) -> contact_snapshot_model::Observation {
    contact_snapshot_model::Observation {
        identity: actor.identity,
        percent_bits: Some(percent.to_bits()),
    }
}
fn fixture(weapon: bool) -> (feed::Contact, feed::Update) {
    let source = actor(0);
    let target = actor(1);
    let owner = observation(source, 0.0);
    let attack = if weapon {
        contact_snapshot_model::Observation {
            identity: contact_snapshot_model::Identity {
                object_id: 0x10000001,
                accessor: 0x3000,
                category: 1,
                kind: 4,
                entry: -1,
                founder: Some(0),
            },
            percent_bits: None,
        }
    } else {
        owner
    };
    let contact = feed::Contact {
        raw: damage_capture_model::RawSample {
            epoch: 1,
            frame: 10,
            object_id_1: attack.identity.object_id,
            object_id_2: 1,
            amount_bits: 5f32.to_bits(),
            caller_return_offset: Some(0x6c2f10),
            ..Default::default()
        },
        objects: [
            Some(attack),
            Some(observation(target, 10.0)),
            weapon.then_some(owner),
        ],
        actors: [Some(source), Some(target)],
        statuses: [Some(0), Some(1)],
    };
    let update = feed::Update {
        sample: damage_commit_model::Sample {
            epoch: 1,
            frame: 10,
            index: 0,
            context_id: attack.identity.object_id,
            caller_return_offset: Some(0x3febd8),
            before_bits: 10f32.to_bits(),
            pending_bits: 5f32.to_bits(),
            after_bits: 15f32.to_bits(),
            cap_bits: Some(999f32.to_bits()),
            suppressed: false,
            objects: [
                Some(attack),
                Some(observation(target, 15.0)),
                weapon.then_some(owner),
            ],
        },
        actors: contact.actors,
    };
    (contact, update)
}
fn stamp(frame: u64) -> feed::Stamp {
    feed::Stamp {
        epoch: 1,
        frame,
        revision: 0,
    }
}
fn paired(
    contact: feed::Contact,
    update: feed::Update,
    current: impl FnMut(feed::Actor) -> bool,
) -> feed::Decision {
    let mut queue = feed::Queue::new();
    queue.begin(stamp(10)).unwrap();
    let ticket = queue.reserve(stamp(10), feed::Stream::Contact).unwrap();
    queue.contact(ticket, contact).unwrap();
    let ticket = queue.reserve(stamp(10), feed::Stream::Update).unwrap();
    queue.update(ticket, update).unwrap();
    queue.seal(stamp(11)).unwrap();
    queue.next(stamp(11), current).unwrap().unwrap()
}
#[test]
fn direct_and_owned_weapon_credit_require_exact_current_stocks_and_owner() {
    for weapon in [false, true] {
        let (contact, update) = fixture(weapon);
        let feed::Decision::Candidate(candidate) = paired(contact, update, |_| true) else {
            panic!("valid contact rejected")
        };
        assert_eq!(candidate.weapon.is_some(), weapon);
        assert_eq!(candidate.source, actor(0));
        assert_eq!(candidate.amount.unwrap().value(), 5.0);
        assert!(matches!(
            paired(contact, update, |a| a != actor(0)),
            feed::Decision::Excluded {
                reason: feed::Exclusion::Lifetime,
                ..
            }
        ));
        let mut stale = update;
        stale.actors[0].as_mut().unwrap().stock_generation += 1;
        assert!(matches!(
            paired(contact, stale, |_| true),
            feed::Decision::Excluded {
                reason: feed::Exclusion::Lifetime,
                ..
            }
        ));
        if weapon {
            let mut wrong = contact;
            wrong.objects[2].as_mut().unwrap().identity = actor(2).identity;
            assert!(matches!(
                paired(wrong, update, |_| true),
                feed::Decision::Excluded {
                    reason: feed::Exclusion::WeaponCredit,
                    ..
                }
            ));
            let mut self_hit = contact;
            self_hit.actors[0] = self_hit.actors[1];
            self_hit.objects[0].as_mut().unwrap().identity.founder = Some(1);
            self_hit.objects[2] = Some(observation(actor(1), 10.0));
            let mut self_update = update;
            self_update.actors = self_hit.actors;
            self_update.sample.objects[0] = self_hit.objects[0];
            self_update.sample.objects[2] = self_hit.objects[2];
            assert!(matches!(
                paired(self_hit, self_update, |_| true),
                feed::Decision::Excluded {
                    reason: feed::Exclusion::SelfContact,
                    ..
                }
            ));
        }
        let mut zero = update;
        zero.sample.pending_bits = 0f32.to_bits();
        assert!(matches!(
            paired(contact, zero, |_| true),
            feed::Decision::Excluded { .. }
        ));
    }
}
#[test]
fn delivery_preserves_distinct_contacts_and_rejects_replay() {
    let (contact, update) = fixture(false);
    let feed::Decision::Candidate(mut candidate) = paired(contact, update, |_| true) else {
        panic!()
    };
    let mut cursor = damage_delivery_model::Cursor::new();
    let events = cursor.begin(candidate, stamp(11)).unwrap();
    assert_eq!(events[0].receiver(), actor(0));
    assert_eq!(events[1].receiver(), actor(1));
    cursor.finish();
    candidate.contact_index += 1;
    candidate.update_index += 1;
    assert!(cursor.begin(candidate, stamp(11)).is_ok());
    cursor.finish();
    assert_eq!(
        cursor.begin(candidate, stamp(11)),
        Err(damage_delivery_model::Rejection::Consumed)
    );
    candidate.target = actor(2);
    candidate.contact_index += 1;
    candidate.update_index += 1;
    assert!(cursor.begin(candidate, stamp(11)).is_ok());
    cursor.finish();
    candidate.stamp.frame += 1;
    candidate.contact_index = 0;
    candidate.target = actor(1);
    assert!(cursor.begin(candidate, stamp(12)).is_ok());
    cursor.finish();
    candidate.target = candidate.source;
    assert_eq!(
        cursor.begin(candidate, stamp(12)),
        Err(damage_delivery_model::Rejection::Invalid)
    );
    candidate.target = actor(1);
    candidate.stamp.revision += 1;
    assert_eq!(
        cursor.begin(candidate, stamp(12)),
        Err(damage_delivery_model::Rejection::Lifetime)
    );
}

#[test]
fn weapon_founder_alone_cannot_grant_reflected_or_transferred_credit() {
    use contact_snapshot_model::fighter_owner;
    assert_eq!(fighter_owner(0, 0), Some(0));
    assert_eq!(fighter_owner(65536, 65536), Some(65536));
    for (founder, owner) in [(0, 1), (1, 0), (-1, -1), (0, -1), (0x10000000, 0x10000000)] {
        assert_eq!(fighter_owner(founder, owner), None);
    }
    let native = include_str!("../../plugins/slight_replica/src/slight/gimmicks/damage_capture.rs");
    assert!(native.contains("WEAPON_INSTANCE_WORK_ID_INT_LINK_OWNER"));
    assert!(native.contains("contact::fighter_owner(id, owner)"));
}

#[test]
fn rounded_nonpositive_pending_never_becomes_a_damage_event() {
    let (_, mut update) = fixture(false);
    update.sample.after_bits = update.sample.before_bits;
    update.sample.objects[1].as_mut().unwrap().percent_bits = Some(update.sample.before_bits);
    for amount in [0.0, -f32::MIN_POSITIVE, f32::NAN, f32::INFINITY] {
        update.sample.pending_bits = amount.to_bits();
        assert!(damage_commit_model::confirmed_pending(&update.sample).is_none());
    }
    update.sample.pending_bits = f32::MIN_POSITIVE.to_bits();
    assert!(damage_commit_model::confirmed_pending(&update.sample).is_some());
}

#[test]
fn plugin_routes_reprove_both_fighters_and_capture_weapon_before_static_or_live_actions() {
    let feed = include_str!("../../plugins/slight_replica/src/slight/gimmicks/damage_feed.rs");
    assert!(!feed.contains("read_native_object(weapon.object_id)"));
    let commit = include_str!("../../plugins/slight_replica/src/slight/gimmicks/damage_commit.rs");
    assert!(commit.contains("read_native_object(before.identity.object_id)"));
    assert!(commit.contains("after.identity == before.identity"));
    assert!(feed.contains("[row.source, row.target]"));
    let context =
        include_str!("../../plugins/slight_replica/src/slight/gimmicks/damage_context.rs");
    assert!(context.contains(".with_projectile(event.contact().weapon.is_some())"));
    for source in [
        include_str!("../../plugins/slight_replica/src/slight/gimmicks/live_behavior.rs"),
        include_str!(
            "../../plugins/slight_replica/src/slight/gimmicks/live_behavior/controllers.rs"
        ),
    ] {
        assert!(source.contains("damage_context::"));
    }
}

#[path = "../../plugins/slight_replica/src/slight/gimmicks/shield_contacts_model.rs"]
mod shield_contacts_model;

#[test]
fn shield_roots_round_trip_and_require_explicit_contact_context() {
    for (hook, direction, live_hook) in [
        (
            HookName::OnAttackShielded,
            DamageDirection::AttackShielded,
            m::HookName::OnAttackShielded,
        ),
        (
            HookName::OnShieldHit,
            DamageDirection::ShieldHit,
            m::HookName::OnShieldHit,
        ),
    ] {
        let mut graph = graph(hook);
        graph.transitions[0].actions = vec![Action::Set {
            var: "amount".into(),
            value: Expr::Const(ConstValue::Float(7.0)),
        }];
        assert!(validate_for_fighter("cloud", &graph).is_empty());
        let program = super::live_protocol::program_from_graph("cloud", &graph).unwrap();
        assert_eq!(program.bindings[0].hook, live_hook);
        let wire = serde_json::to_string(&program).unwrap();
        let decoded: m::LiveProgram = serde_json::from_str(&wire).unwrap();
        assert_eq!(decoded, program);
        decoded.validate().unwrap();
        for projectile in [false, true] {
            let mut state = program.initial_state();
            let input = DamageInput::shield(direction)
                .unwrap()
                .with_projectile(projectile);
            let result = program
                .step_damage_with_snapshot(&mut state, input, &NativeSnapshot::empty())
                .unwrap();
            assert_eq!(result.fired.len(), usize::from(projectile));
        }
        assert!(program
            .step(&mut program.initial_state(), live_hook)
            .is_err());
        let rendered = super::adapter::render(&std::collections::BTreeMap::from([(
            "cloud".into(),
            graph.clone(),
        )]))
        .unwrap();
        let encoded = rendered
            .source
            .lines()
            .find_map(|line| line.strip_prefix("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = "))
            .unwrap();
        let encoded: String = serde_json::from_str(encoded.strip_suffix(';').unwrap()).unwrap();
        let bundle = super::controller_protocol::decode_request(&encoded).unwrap();
        assert_eq!(bundle.charts[0].program, program);
        graph
            .emission_plan
            .bindings
            .push(super::codegen::HookBinding {
                hook: HookName::OnChargeTick,
                article: None,
                transitions: vec![0],
            });
        assert!(!validate(&graph).is_empty());
    }
}

#[test]
fn missing_blocked_damage_is_explicit_and_never_becomes_zero_or_percent_damage() {
    for (hook, direction) in [
        (HookName::OnAttackShielded, DamageDirection::AttackShielded),
        (HookName::OnShieldHit, DamageDirection::ShieldHit),
    ] {
        let program = super::live_protocol::program_from_graph("cloud", &graph(hook)).unwrap();
        let input = DamageInput::shield(direction)
            .unwrap()
            .with_projectile(true);
        let mut state = program.initial_state();
        assert!(program
            .step_damage_with_snapshot(&mut state, input, &NativeSnapshot::empty())
            .is_err());
        assert_eq!(state, program.initial_state());
        let input = DamageInput::new(direction, 4.5)
            .unwrap()
            .with_projectile(true);
        program
            .step_damage_with_snapshot(&mut state, input, &NativeSnapshot::empty())
            .unwrap();
        assert_eq!(state.values["amount"], m::Value::Float(4.5));
    }
    assert!(DamageInput::shield(DamageDirection::Deal).is_err());
    assert!(DamageInput::shield(DamageDirection::Take).is_err());
}

#[test]
fn each_shield_contact_routes_both_sides_even_for_same_attacker_and_victim() {
    let mut queue = shield_contacts_model::Queue::new();
    for _ in 0..3 {
        assert!(queue.push(
            stamp(10),
            [actor(0), actor(1)],
            [Some(0), Some(30)],
            None,
            None
        ));
    }
    let rows = queue.drain(stamp(11));
    let mut cursor = damage_delivery_model::Cursor::new();
    for (index, row) in rows.into_iter().flatten().enumerate() {
        assert_eq!(row.contact_index, index);
        let events = cursor.begin(row, stamp(11)).unwrap();
        assert_eq!(
            events[0].direction(),
            damage_delivery_model::Direction::AttackShielded
        );
        assert_eq!(
            events[1].direction(),
            damage_delivery_model::Direction::ShieldHit
        );
        assert_eq!(events[0].receiver(), actor(0));
        assert_eq!(events[1].receiver(), actor(1));
        assert_eq!(events[1].receiver_status_at_contact(), Some(30));
        assert_eq!(events[1].amount(), None);
        cursor.finish();
        assert_eq!(
            cursor.begin(row, stamp(11)),
            Err(damage_delivery_model::Rejection::Consumed)
        );
    }
    assert!(queue.drain(stamp(11)).iter().all(Option::is_none));
}

#[test]
fn shield_queue_rejects_incomplete_or_stale_frames_and_preserves_weapon_credit() {
    for changed in [
        feed::Stamp {
            epoch: 2,
            ..stamp(11)
        },
        feed::Stamp {
            revision: 1,
            ..stamp(11)
        },
        stamp(12),
    ] {
        let mut queue = shield_contacts_model::Queue::new();
        assert!(queue.push(stamp(10), [actor(0), actor(1)], [None, None], None, None));
        assert!(queue.drain(changed).iter().all(Option::is_none));
    }
    let mut queue = shield_contacts_model::Queue::new();
    for _ in 0..feed::CAPACITY {
        assert!(queue.push(stamp(10), [actor(0), actor(1)], [None, None], None, None));
    }
    assert!(!queue.push(stamp(10), [actor(0), actor(1)], [None, None], None, None));
    assert!(queue.drain(stamp(11)).iter().all(Option::is_none));
    let (contact, _) = fixture(true);
    let weapon = contact.objects[0].map(|v| v.identity);
    assert!(queue.push(
        stamp(11),
        [actor(0), actor(1)],
        [None, None],
        weapon,
        Some(5f32.to_bits())
    ));
    let row = queue.drain(stamp(12))[0].unwrap();
    let mut cursor = damage_delivery_model::Cursor::new();
    let events = cursor.begin(row, stamp(12)).unwrap();
    assert_eq!(events[0].contact().weapon, weapon);
    assert_eq!(events[1].amount().unwrap().value(), 5.0);
    cursor.finish();
    let mut bad = row;
    bad.contact_index += 1;
    bad.weapon.as_mut().unwrap().founder = Some(2);
    assert_eq!(
        cursor.begin(bad, stamp(12)),
        Err(damage_delivery_model::Rejection::Invalid)
    );
}

#[test]
fn shield_runtime_uses_native_contacts_and_shared_checked_dispatch() {
    let native =
        include_str!("../../plugins/slight_replica/src/slight/gimmicks/shield_contacts.rs");
    for required in [
        "0x3df93c",
        "COLLISION_KIND_MASK_SHIELD",
        "ctx.registers[8].w()",
        "ctx.registers[19].x()",
        "ctx.registers[22].x()",
        "published_actor_with_status",
        "damage_feed::delivery_is_current",
        "damage_dispatch::deliver",
        "Some(attack.identity)",
    ] {
        assert!(native.contains(required), "{required}");
    }
    assert!(!native.contains("is_infliction_status("));
    assert!(!native.contains("ATTACK_SERIAL"));
    let cursor =
        include_str!("../../plugins/slight_replica/src/slight/gimmicks/damage_delivery_model.rs");
    assert!(!cursor.contains("victims.contains"));
    assert!(!cursor.contains("ATTACK_SERIAL"));
}

#[test]
fn blocked_native_power_is_bounded_and_zero_is_a_real_payload() {
    for amount in [0.0f32, 4.5, 1_000_000.0] {
        let value =
            damage_commit_model::NativeDamageAmount::blocked_power(amount.to_bits()).unwrap();
        assert_eq!(value.value(), amount);
        for direction in [DamageDirection::AttackShielded, DamageDirection::ShieldHit] {
            assert_eq!(
                DamageInput::new(direction, value.value())
                    .unwrap()
                    .pending(),
                Some(amount)
            );
        }
    }
    for amount in [-1.0f32, f32::NAN, f32::INFINITY, 1_000_001.0] {
        assert!(damage_commit_model::NativeDamageAmount::blocked_power(amount.to_bits()).is_none());
    }
}

#[test]
fn incoming_hit_cancel_is_scoped_and_packaged_with_its_payload() {
    assert!(DamageInput::new(DamageDirection::Incoming, 0.0).is_ok());
    for power in [-1.0, f32::NAN, f32::INFINITY] {
        assert!(DamageInput::new(DamageDirection::Incoming, power).is_err());
    }
    let mut graph = graph(HookName::OnIncomingHit);
    graph.transitions[0].actions.push(Action::CancelIncomingHit);
    assert!(validate_for_fighter("cloud", &graph).is_empty());
    let program = super::live_protocol::program_from_graph("cloud", &graph).unwrap();
    for projectile in [false, true] {
        let mut state = program.initial_state();
        let input = DamageInput::new(DamageDirection::Incoming, 13.5)
            .unwrap()
            .with_projectile(projectile);
        let result = program
            .step_damage_with_snapshot(&mut state, input, &NativeSnapshot::empty())
            .unwrap();
        assert_eq!(
            result.engine_actions,
            if projectile {
                vec![m::Action::CancelIncomingHit]
            } else {
                vec![]
            }
        );
        assert_eq!(
            state.values["amount"],
            m::Value::Float(if projectile { 13.5 } else { 2.0 })
        );
    }
    assert!(program
        .step(&mut program.initial_state(), m::HookName::OnIncomingHit)
        .is_err());
    let rendered = super::adapter::render(&std::collections::BTreeMap::from([(
        "cloud".into(),
        graph.clone(),
    )]))
    .unwrap();
    let encoded = rendered
        .source
        .lines()
        .find_map(|line| line.strip_prefix("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = "))
        .unwrap();
    let encoded: String = serde_json::from_str(encoded.strip_suffix(';').unwrap()).unwrap();
    assert_eq!(
        super::controller_protocol::decode_request(&encoded)
            .unwrap()
            .charts[0]
            .program,
        program
    );
    for hook in [
        HookName::OnTakeDamage,
        HookName::OnShieldHit,
        HookName::OnChargeTick,
    ] {
        let mut invalid = graph.clone();
        invalid.emission_plan.bindings[0].hook = hook;
        assert!(!validate_for_fighter("cloud", &invalid).is_empty());
        let mut invalid = program.clone();
        invalid.bindings[0].hook = match hook {
            HookName::OnTakeDamage => m::HookName::OnTakeDamage,
            HookName::OnShieldHit => m::HookName::OnShieldHit,
            _ => m::HookName::OnChargeTick,
        };
        assert!(invalid.validate().is_err());
    }
}

#[test]
fn incoming_window_samples_status_and_held_input_at_the_contact() {
    let mut graph = graph(HookName::OnIncomingHit);
    graph.transitions[0].on_frame = false;
    graph.transitions[0].guard = Predicate::And(
        Box::new(Predicate::InStatus {
            symbol: "FIGHTER_STATUS_KIND_WAIT".into(),
        }),
        Box::new(Predicate::Button {
            button: InputButton::Guard,
            edge: ButtonEdge::Held,
        }),
    );
    graph.transitions[0].actions = vec![
        Action::CancelIncomingHit,
        Action::RequestFighterStatus {
            symbol: "FIGHTER_STATUS_KIND_SPECIAL_HI".into(),
        },
    ];
    assert!(validate_for_fighter("cloud", &graph).is_empty());
    let program = super::live_protocol::program_from_graph("cloud", &graph).unwrap();
    assert!(program
        .required_native_features()
        .contains(INCOMING_HIT_FEATURE));
    for (status, held, cancels) in [(0, true, true), (0, false, false), (1, true, false)] {
        let mut snapshot = NativeSnapshot::empty();
        snapshot.phase = Some(PhaseInput {
            fighter: "cloud".into(),
            status,
            status_frame: 0,
            motion_frame: 0.0,
            motion: 0,
            situation: Some(SituationKind::Ground),
        });
        snapshot.controller = Some(ControllerInput {
            buttons: std::collections::BTreeMap::from([(
                (InputButton::Guard, ButtonEdge::Held),
                held,
            )]),
            ..Default::default()
        });
        let input = DamageInput::new(DamageDirection::Incoming, 8.0).unwrap();
        let result = program
            .step_damage_with_snapshot(&mut program.initial_state(), input, &snapshot)
            .unwrap();
        assert_eq!(result.engine_actions.len(), if cancels { 2 } else { 0 });
    }
    let input = DamageInput::new(DamageDirection::Incoming, 8.0).unwrap();
    assert!(program
        .step_damage_with_snapshot(
            &mut program.initial_state(),
            input,
            &NativeSnapshot::empty()
        )
        .is_err());
}

#[path = "../../plugins/slight_replica/src/slight/gimmicks/receiver_deflect_model.rs"]
mod receiver_deflect_model;
use super::native_inputs;

#[test]
fn receiver_deflect_queue_preserves_each_contact_and_fences_batches() {
    use receiver_deflect_model::{Contact, Queue};
    let stamp = feed::Stamp {
        epoch: 1,
        frame: 5,
        revision: 2,
    };
    let actor = feed::Actor {
        identity: contact_snapshot_model::Identity {
            object_id: 7,
            accessor: 0x1000,
            category: 0,
            kind: 7,
            entry: 1,
            founder: None,
        },
        stock_generation: 3,
    };
    let row = Contact {
        actor,
        stamp,
        input: DamageInput::new(DamageDirection::Reflect, 6.5).unwrap(),
    };
    let next = feed::Stamp { frame: 6, ..stamp };
    let mut queue = Queue::new();
    for _ in 0..feed::CAPACITY {
        queue.push(row);
    }
    let rows = queue.drain(next);
    assert_eq!(rows.iter().flatten().count(), feed::CAPACITY);
    assert!(rows
        .into_iter()
        .flatten()
        .all(|r| r.actor == actor && r.input.pending() == Some(6.5)));
    assert!(queue.drain(next).iter().all(Option::is_none));
    for invalid in [
        stamp,
        feed::Stamp { frame: 7, ..next },
        feed::Stamp { epoch: 2, ..next },
        feed::Stamp {
            revision: 3,
            ..next
        },
    ] {
        queue.push(row);
        assert!(queue.drain(invalid).iter().all(Option::is_none));
    }
    for _ in 0..=feed::CAPACITY {
        queue.push(row);
    }
    assert!(queue.drain(next).iter().all(Option::is_none));
    queue.push(row);
    queue.push(Contact {
        stamp: feed::Stamp {
            revision: 3,
            ..stamp
        },
        ..row
    });
    assert!(queue.drain(next).iter().all(Option::is_none));
}

#[test]
fn reflect_absorb_roots_have_scoped_power_and_static_live_parity() {
    for (hook, direction, live) in [
        (
            HookName::OnReflect,
            DamageDirection::Reflect,
            m::HookName::OnReflect,
        ),
        (
            HookName::OnAbsorb,
            DamageDirection::Absorb,
            m::HookName::OnAbsorb,
        ),
    ] {
        assert!(super::hooks::resolve(hook.name()).is_ok());
        assert!(super::graph_edit::direct_hooks().contains(&hook));
        let graph = graph(hook);
        let program = super::live_protocol::program_from_graph("cloud", &graph).unwrap();
        assert!(program
            .required_native_features()
            .contains(REFLECT_ABSORB_FEATURE));
        assert!(super::behavior::required_native_features(&graph).contains(REFLECT_ABSORB_FEATURE));
        assert!(super::behavior::requires_controller_runtime(&graph));
        let mut state = program.initial_state();
        let input = DamageInput::new(direction, 9.5)
            .unwrap()
            .with_projectile(true);
        program
            .step_damage_with_snapshot(&mut state, input, &NativeSnapshot::empty())
            .unwrap();
        assert_eq!(state.values["amount"], m::Value::Float(9.5));
        assert!(program.step(&mut state, live).is_err());
        let mut mixed = graph.clone();
        mixed
            .emission_plan
            .bindings
            .push(super::codegen::HookBinding {
                hook: HookName::OnChargeTick,
                article: None,
                transitions: vec![0],
            });
        assert!(!validate(&mixed).is_empty());
        let mut cancel = graph.clone();
        cancel.transitions[0].actions = vec![Action::CancelIncomingHit];
        assert!(!validate(&cancel).is_empty());
        let rendered =
            super::adapter::render(&std::collections::BTreeMap::from([("cloud".into(), graph)]))
                .unwrap();
        let encoded = rendered
            .source
            .lines()
            .find_map(|line| line.strip_prefix("const VISIONARY_STATIC_CONTROLLER_BUNDLE: &str = "))
            .unwrap();
        let encoded: String = serde_json::from_str(encoded.strip_suffix(';').unwrap()).unwrap();
        let bundle = super::controller_protocol::decode_request(&encoded).unwrap();
        assert_eq!(bundle.charts[0].program, program);
        for bad in [-1.0, f32::NAN, f32::INFINITY] {
            assert!(DamageInput::new(direction, bad).is_err());
        }
        assert!(DamageInput::new(direction, 0.0).is_ok());
        let absent = DamageInput::shield(direction)
            .unwrap()
            .with_projectile(true);
        assert!(program
            .step_damage_with_snapshot(
                &mut program.initial_state(),
                absent,
                &NativeSnapshot::empty()
            )
            .is_err());
        let mut without_power = program.clone();
        without_power.transitions[0].actions = vec![m::Action::Set {
            var: "amount".into(),
            value: m::Expr::Const(m::Value::Float(7.0)),
        }];
        let mut state = without_power.initial_state();
        without_power
            .step_damage_with_snapshot(&mut state, absent, &NativeSnapshot::empty())
            .unwrap();
        assert_eq!(state.values["amount"], m::Value::Float(7.0));
    }
}
