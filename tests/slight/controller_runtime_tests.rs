use super::*;
use slight::gimmicks::controller_protocol as protocol;
use slight::gimmicks::controller_protocol::ControllerProgram;
use slight::gimmicks::live_behavior::ControllerStateRow;
use slight::gimmicks::native_inputs::{FighterValue, NativeInput};

#[test]
fn resource_helpers_deliver_once_to_the_owner_in_compiled_and_live_controllers() {
    use slight::gimmicks::resource_commands::{Command, Op, Resource};
    let _serial = TEST_LOCK.lock().unwrap();
    for resource in [
        "jokerRebelGauge",
        "heroMp",
        "steveDiamonds",
        "littleMacKoGauge",
    ]
    .map(|key| Resource::named(key).unwrap())
    {
        for compiled in [true, false] {
            reset();
            fighter(101, 0x10100, 0, resource.fighter());
            fighter(102, 0x10200, 1, "samus");
            let command = Command {
                op: Op::Add,
                resource,
                amount: 5.0,
            };
            let authored_command = Command {
                op: Op::Add,
                resource,
                amount: model::Expr::Const(model::Value::Float(5.0)),
            };
            let mut authored =
                ordinary_action_program(0, vec![model::Action::Resource(authored_command)]);
            authored.fighter = resource.fighter().into();
            if compiled {
                install_compiled(vec![("main", authored)]);
            } else {
                install_controller(1, vec![("main", authored)]);
            }
            runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 102);
            assert!(slight::gimmicks::resource_control::take_calls().is_empty());
            runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 101);
            assert_eq!(
                slight::gimmicks::resource_control::take_calls(),
                vec![(101, command)]
            );
            let identity = slight::gimmicks::actor_identity::Identity::from_live_fighter(
                101,
                resource.fighter(),
            )
            .unwrap();
            smash::app::sv_battle_object::set(101, 0xDEAD, 0, identity.fighter_kind, 0);
            assert!(slight::gimmicks::resource_control::execute(101, identity, command).is_err());
            runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 101);
            assert!(slight::gimmicks::resource_control::take_calls().is_empty());
        }
    }
}

fn controller_bundle(revision: u64, entries: Vec<(&str, model::LiveProgram)>) -> String {
    let bundle = protocol::ControllerBundleRequest {
        version: protocol::REQUEST_VERSION,
        revision,
        disabled_fighters: Vec::new(),
        charts: entries
            .into_iter()
            .map(|(chart_id, mut authored)| {
                assert!(authored.validate().is_ok(), "{:?}", authored.validate());
                if chart_id != "main" {
                    authored.id = format!("v5:{chart_id}");
                }
                assert!(authored.validate().is_ok(), "{:?}", authored.validate());
                ControllerProgram {
                    chart_id: chart_id.into(),
                    program: authored,
                }
            })
            .collect(),
    };
    assert!(
        protocol::validate_request(&bundle).is_ok(),
        "{:?}",
        protocol::validate_request(&bundle)
    );
    protocol::encode_request(&bundle).unwrap()
}

pub(super) fn install_controller(revision: u64, entries: Vec<(&str, model::LiveProgram)>) -> u64 {
    let raw = controller_bundle(revision, entries);
    let ack = runtime::handle(&raw);
    assert!(ack.accepted, "{}", ack.message);
    ack.revision
}

pub(super) fn install_compiled(entries: Vec<(&str, model::LiveProgram)>) -> String {
    let raw = controller_bundle(0, entries);
    runtime::test_install_compiled_baseline(&raw).unwrap();
    raw
}

#[test]
fn character_value_reads_follow_actor_chart_and_effective_controller_without_dispatch() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(106, 0x10600, 0);
    cloud(107, 0x10700, 1);
    install_compiled(vec![
        ("main", named_add_program(1, 1)),
        ("meter_1", named_typed_add_program(10)),
    ]);
    let read = runtime::read_controller_value;
    assert_eq!(
        read(106, "cloud", "main", "count"),
        Some(model::Value::Int(1))
    );
    assert_eq!(
        read(106, "cloud", "meter_1", "timer"),
        Some(model::Value::Int(5))
    );
    assert_eq!(
        read(106, "cloud", "meter_1", "ratio_seen"),
        Some(model::Value::Float(0.0))
    );
    assert_eq!(
        read(106, "cloud", "meter_1", "enabled_seen"),
        Some(model::Value::Flag(false))
    );
    assert_eq!(runtime::test_actor_slot_counts(), (0, 0, 0));
    assert_eq!(slight::gimmicks::native_values::take_sample_count(), 0);
    slight::gimmicks::chart_event_bridge::dispatch_values(
        106,
        "meter_1",
        "external_action",
        &[
            ("scale", model::Value::Float(1.5)),
            ("amount", model::Value::Int(3)),
            ("enabled", model::Value::Flag(true)),
        ],
    )
    .unwrap();
    assert_eq!(
        read(106, "cloud", "meter_1", "count"),
        Some(model::Value::Int(13))
    );
    assert_eq!(
        read(106, "cloud", "meter_1", "ratio_seen"),
        Some(model::Value::Float(1.5))
    );
    assert_eq!(
        read(106, "cloud", "meter_1", "enabled_seen"),
        Some(model::Value::Flag(true))
    );
    assert_eq!(
        read(107, "cloud", "meter_1", "count"),
        Some(model::Value::Int(10))
    );
    assert_eq!(read(106, "cloud", "missing", "count"), None);
    assert_eq!(read(106, "cloud", "meter_1", "missing"), None);
    assert_eq!(read(106, "samus", "meter_1", "count"), None);
    cloud(106, 0x10610, 0);
    assert_eq!(
        read(106, "cloud", "meter_1", "count"),
        Some(model::Value::Int(10))
    );
    install_controller(1, vec![("main", named_add_program(40, 5))]);
    assert_eq!(
        read(106, "cloud", "main", "count"),
        Some(model::Value::Int(40))
    );
    assert_eq!(read(106, "cloud", "meter_1", "count"), None);
    assert!(runtime::handle(&disabled_bundle(2, "cloud")).accepted);
    assert_eq!(read(106, "cloud", "main", "count"), None);
}

#[test]
fn source_chart_bridge_targets_exact_actor_chart_and_event_without_main_fallback() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(101, 0x10100, 0);
    install_compiled(vec![
        ("main", named_add_program(1, 1)),
        ("left", named_add_program(10, 2)),
        ("right", named_add_program(100, 20)),
    ]);
    let dispatch = slight::gimmicks::chart_event_bridge::dispatch;
    dispatch(101, "left", "external_action").unwrap();
    dispatch(101, "left", "external_action").unwrap();
    assert_eq!(controller_count(101, "left"), 14);
    assert_eq!(controller_count(101, "right"), 100);
    assert_eq!(controller_count(101, "main"), 1);
    let prior = runtime::controller_wire_rows();
    assert!(dispatch(101, "missing", "external_action").is_err());
    assert!(dispatch(101, "right", "missing").is_err());
    assert!(dispatch(101, "main", "external_action").is_err());
    assert_eq!(runtime::controller_wire_rows(), prior);
}

#[test]
fn source_chart_bridge_obeys_live_mask_and_never_selects_hidden_baseline() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(102, 0x10200, 0);
    install_compiled(vec![("extra", named_add_program(1, 1))]);
    let dispatch = slight::gimmicks::chart_event_bridge::dispatch;
    dispatch(102, "extra", "external_action").unwrap();
    assert_eq!(controller_count(102, "extra"), 2);
    install_controller(1, vec![("main", named_add_program(40, 5))]);
    assert!(dispatch(102, "extra", "external_action").is_err());
    assert_eq!(controller_count(102, "main"), 40);
    assert!(runtime::controller_wire_rows()
        .iter()
        .all(|row| row.chart_id == "main"));
    let cleared = runtime::handle(&controller_bundle(2, vec![]));
    assert!(cleared.accepted);
    dispatch(102, "extra", "external_action").unwrap();
    assert_eq!(controller_count(102, "extra"), 2);
    assert!(runtime::handle(&disabled_bundle(3, "cloud")).accepted);
    assert!(dispatch(102, "extra", "external_action").is_err());
    assert!(runtime::controller_wire_rows().is_empty());
}

#[test]
fn source_chart_bridge_rejects_invalid_identifiers_wrong_owner_and_stale_identity() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(103, 0x10300, 0);
    install_compiled(vec![("extra", named_add_program(1, 1))]);
    let dispatch = slight::gimmicks::chart_event_bridge::dispatch;
    for chart in ["", "  ", "main", "bad\0id", &"x".repeat(49)] {
        assert!(dispatch(103, chart, "external_action").is_err());
    }
    for event in [
        "",
        "  ",
        "bad\0name",
        &"x".repeat(model::MAX_NAME_BYTES + 1),
    ] {
        assert!(dispatch(103, "extra", event).is_err());
    }
    assert!(dispatch(999, "extra", "external_action").is_err());
    // A registered Mario must not select Cloud's same-named chart.
    let mut changed = slight::agents::lookup(103).unwrap();
    changed.kind = 2;
    slight::agents::set(changed);
    app::sv_battle_object::set(103, 0x10300, 0, 2, 0);
    assert!(dispatch(103, "extra", "external_action").is_err());
    cloud(103, 0x10300, 0);
    app::sv_battle_object::set(103, 0xDEAD, 0, 1, 0);
    assert!(dispatch(103, "extra", "external_action").is_err());
    assert_eq!(runtime::test_actor_slot_counts().0, 0);
}

#[test]
fn source_chart_bridge_refuses_payload_receiver_before_actor_reservation() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(104, 0x10400, 0);
    let mut typed = named_add_program(10, 2);
    typed.event_bindings[0].fields.push(model::EventField {
        name: "amount".into(),
        kind: model::VarKind::Int,
    });
    install_compiled(vec![("extra", typed)]);
    let error = slight::gimmicks::chart_event_bridge::dispatch(104, "extra", "external_action")
        .unwrap_err();
    assert!(error.contains("typed arguments"));
    assert_eq!(runtime::test_actor_slot_counts(), (0, 0, 0));
    assert_eq!(slight::gimmicks::native_values::take_sample_count(), 0);
}

#[test]
fn source_chart_values_bridge_targets_one_exact_chart_with_all_scalar_types() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(105, 0x10500, 0);
    install_compiled(vec![
        ("main", named_typed_add_program(0)),
        ("left", named_typed_add_program(0)),
        ("right", named_typed_add_program(0)),
    ]);
    let dispatch = slight::gimmicks::chart_event_bridge::dispatch_values;
    let arguments = [
        ("scale", model::Value::Float(1.5)),
        ("amount", model::Value::Int(3)),
        ("enabled", model::Value::Flag(true)),
    ];
    let timers = [
        controller_timer(105, "main"),
        controller_timer(105, "left"),
        controller_timer(105, "right"),
    ];

    for _ in 0..3 {
        dispatch(105, "right", "external_action", &arguments).unwrap();
    }

    assert_eq!(controller_count(105, "main"), 0);
    assert_eq!(controller_count(105, "left"), 0);
    assert_eq!(controller_count(105, "right"), 9);
    let right = controller_row(105, "right");
    assert_eq!(right.actor.vars["ratio_seen"], model::Value::Float(1.5));
    assert_eq!(right.actor.vars["enabled_seen"], model::Value::Flag(true));
    for chart_id in ["main", "left"] {
        let row = controller_row(105, chart_id);
        assert_eq!(row.actor.vars["ratio_seen"], model::Value::Float(0.0));
        assert_eq!(row.actor.vars["enabled_seen"], model::Value::Flag(false));
    }
    assert_eq!(
        [
            controller_timer(105, "main"),
            controller_timer(105, "left"),
            controller_timer(105, "right"),
        ],
        timers,
        "external events must not tick any chart timer"
    );
    assert_eq!(slight::gimmicks::native_values::take_sample_count(), 3);
}

#[test]
fn source_chart_values_reject_bad_payloads_before_actor_reservation_or_sampling() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(106, 0x10600, 0);
    install_compiled(vec![("extra", named_typed_add_program(0))]);
    let dispatch = slight::gimmicks::chart_event_bridge::dispatch_values;
    let valid = [
        ("scale", model::Value::Float(1.5)),
        ("amount", model::Value::Int(3)),
        ("enabled", model::Value::Flag(true)),
    ];
    let wrong_name = [
        ("other", model::Value::Float(1.5)),
        ("amount", model::Value::Int(3)),
        ("enabled", model::Value::Flag(true)),
    ];
    let wrong_order = [
        ("amount", model::Value::Int(3)),
        ("scale", model::Value::Float(1.5)),
        ("enabled", model::Value::Flag(true)),
    ];
    let wrong_type = [
        ("scale", model::Value::Int(1)),
        ("amount", model::Value::Int(3)),
        ("enabled", model::Value::Flag(true)),
    ];
    let non_finite = [
        ("scale", model::Value::Float(f32::NAN)),
        ("amount", model::Value::Int(3)),
        ("enabled", model::Value::Flag(true)),
    ];
    let out_of_range = [
        ("scale", model::Value::Float(1_000_001.0)),
        ("amount", model::Value::Int(3)),
        ("enabled", model::Value::Flag(true)),
    ];
    let missing = &valid[..2];
    let extra = [
        valid[0],
        valid[1],
        valid[2],
        ("extra", model::Value::Int(1)),
    ];
    let oversized = vec![("amount", model::Value::Int(3)); 9];

    for arguments in [
        wrong_name.as_slice(),
        wrong_order.as_slice(),
        wrong_type.as_slice(),
        non_finite.as_slice(),
        out_of_range.as_slice(),
        missing,
        extra.as_slice(),
        oversized.as_slice(),
    ] {
        assert!(dispatch(106, "extra", "external_action", arguments).is_err());
        assert_eq!(runtime::test_actor_slot_counts(), (0, 0, 0));
        assert_eq!(slight::gimmicks::native_values::take_sample_count(), 0);
    }
}

fn disabled_bundle(revision: u64, fighter: &str) -> String {
    let bundle = protocol::ControllerBundleRequest {
        version: protocol::REQUEST_VERSION,
        revision,
        disabled_fighters: vec![fighter.into()],
        charts: Vec::new(),
    };
    protocol::encode_request(&bundle).unwrap()
}

#[test]
fn compiled_baseline_install_is_neutral_idempotent_and_immutable() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(70, 0x7000, 0);
    let base = install_compiled(vec![
        ("main", program(7, false, None, None)),
        ("extra", program(9, false, None, None)),
    ]);
    assert_eq!(runtime::revision(), 0);
    assert_eq!(
        runtime::test_effective_controller_source("cloud"),
        Some("compiled")
    );

    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 70);
    assert_eq!(controller_count(70, "main"), 8);
    assert_eq!(controller_count(70, "extra"), 10);
    runtime::test_install_compiled_baseline(&base).unwrap();
    assert_eq!(
        controller_count(70, "main"),
        8,
        "same boot install must retain actor state"
    );

    let changed = controller_bundle(
        0,
        vec![
            ("main", program(70, false, None, None)),
            ("extra", program(9, false, None, None)),
        ],
    );
    assert!(runtime::test_install_compiled_baseline(&changed)
        .unwrap_err()
        .contains("different compiled controller baseline"));
    assert_eq!(
        controller_count(70, "main"),
        8,
        "rejected replacement must keep old state"
    );
    assert_eq!(
        runtime::revision(),
        0,
        "static install must not advance live revision"
    );

    let mut nonzero_revision: serde_json::Value = serde_json::from_str(&base).unwrap();
    nonzero_revision["gimmick_behavior"]["revision"] = 1.into();
    assert!(runtime::test_install_compiled_baseline(&nonzero_revision.to_string()).is_err());
    assert!(runtime::test_install_compiled_baseline(&disabled_bundle(0, "cloud")).is_err());
    let mut invalid_chart_id: serde_json::Value = serde_json::from_str(&base).unwrap();
    invalid_chart_id["gimmick_behavior"]["charts"][1]["chart_id"] = "  ".into();
    assert!(runtime::test_install_compiled_baseline(&invalid_chart_id.to_string()).is_err());
    let invalid_symbol = controller_bundle(0, vec![("main", named_native_article_program())])
        .replace(
            "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE",
            "FIGHTER_CLOUD_GENERATE_ARTICLE_NOT_REAL",
        );
    assert!(runtime::test_install_compiled_baseline(&invalid_symbol).is_err());
    assert_eq!(controller_count(70, "main"), 8);
}

#[test]
fn live_override_masks_compiled_owner_and_empty_snapshot_restores_fresh_baseline() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(71, 0x7100, 0);
    install_compiled(vec![
        ("main", program(3, false, None, None)),
        ("extra", program(9, false, None, None)),
    ]);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 71);
    assert_eq!(controller_count(71, "main"), 4);

    install_controller(1, vec![("main", program(40, false, None, None))]);
    assert_eq!(
        runtime::test_effective_controller_source("cloud"),
        Some("live")
    );
    assert_eq!(controller_count(71, "main"), 40);
    assert_eq!(
        runtime::test_actor_slot_counts().0,
        0,
        "hidden baseline actor state is discarded"
    );
    assert_eq!(
        runtime::controller_wire_rows().len(),
        1,
        "hidden extra chart is not telemetered"
    );

    install_controller(2, Vec::new());
    assert_eq!(
        runtime::test_effective_controller_source("cloud"),
        Some("compiled")
    );
    assert_eq!(
        controller_count(71, "main"),
        3,
        "restored baseline starts fresh"
    );
    assert_eq!(controller_count(71, "extra"), 9);
    assert_eq!(runtime::revision(), 2);
}

#[test]
fn compiled_named_event_uses_main_only_and_restart_stays_live_revision_gated() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(75, 0x7500, 0);
    install_compiled(vec![
        ("main", named_add_program(2, 5)),
        ("extra", named_add_program(10, 20)),
    ]);
    runtime::dispatch_named(75, "cloud", "external_action")
        .unwrap()
        .unwrap();
    assert_eq!(controller_count(75, "main"), 7);
    assert_eq!(controller_count(75, "extra"), 10);
    let restart = runtime::restart_controller_chart("cloud", 75, "main", 0);
    assert!(!restart.accepted);
    assert!(restart
        .message
        .contains("separate accepted baseline identity"));
}

#[test]
fn compiled_dispatch_owner_override_during_sampling_prevents_any_native_action() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(76, 0x7600, 0);
    install_compiled(vec![("main", baseline_two_effect_named_program())]);
    let live_v4 = request(1, vec![program(50, false, None, None)], Vec::new());
    slight::gimmicks::native_values::replace_snapshot_after_next_sample(live_v4);

    assert!(runtime::dispatch_controller_named(76, "cloud", "main", "external_action").is_err());
    assert!(slight::gimmicks::operations::take_actions().is_empty());
    assert_eq!(slight::gimmicks::native_values::take_sample_count(), 1);
    assert_eq!(runtime::test_actor_slot_counts(), (0, 0, 0));
}

#[test]
fn compiled_dispatch_ignores_unrelated_live_revision_change_during_native_action() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(77, 0x7700, 0);
    install_compiled(vec![("main", baseline_two_effect_named_program())]);
    let mut unrelated = program(0, false, None, None);
    unrelated.fighter = "mario".into();
    unrelated.id = "live:mario".into();
    slight::gimmicks::operations::hook_next_action_replace_snapshot(request(
        1,
        vec![unrelated],
        vec![],
    ));

    runtime::dispatch_controller_named(77, "cloud", "main", "external_action").unwrap();
    assert_eq!(slight::gimmicks::operations::take_actions().len(), 2);
    assert_eq!(slight::gimmicks::native_values::take_sample_count(), 1);
    assert_eq!(controller_count(77, "main"), 1);
    assert_eq!(runtime::revision(), 1);
    assert_eq!(runtime::test_actor_slot_counts(), (1, 0, 1));
}

#[test]
fn compiled_dispatch_cancels_suffix_on_exact_owner_override_disable_clear_or_identity_change() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(78, 0x7800, 0);
    install_compiled(vec![("main", baseline_two_effect_named_program())]);
    let live_v4 = request(1, vec![program(50, false, None, None)], Vec::new());
    slight::gimmicks::operations::hook_next_action_replace_snapshot(live_v4);
    assert!(runtime::dispatch_controller_named(78, "cloud", "main", "external_action").is_ok());
    assert_eq!(slight::gimmicks::operations::take_actions().len(), 1);
    assert_eq!(runtime::test_actor_slot_counts(), (0, 0, 0));

    reset();
    cloud(78, 0x7800, 0);
    install_compiled(vec![("main", baseline_two_effect_named_program())]);
    slight::gimmicks::operations::hook_next_action_replace_snapshot(disabled_bundle(1, "cloud"));
    assert!(runtime::dispatch_controller_named(78, "cloud", "main", "external_action").is_ok());
    assert_eq!(slight::gimmicks::operations::take_actions().len(), 1);
    assert_eq!(runtime::test_actor_slot_counts(), (0, 0, 0));

    reset();
    cloud(78, 0x7800, 0);
    install_compiled(vec![("main", baseline_two_effect_named_program())]);
    slight::gimmicks::operations::hook_next_action_clear_compiled_baseline();
    assert!(runtime::dispatch_controller_named(78, "cloud", "main", "external_action").is_ok());
    assert_eq!(slight::gimmicks::operations::take_actions().len(), 1);
    assert_eq!(runtime::test_actor_slot_counts(), (0, 0, 0));

    reset();
    cloud(78, 0x7800, 0);
    install_compiled(vec![("main", baseline_two_effect_named_program())]);
    slight::gimmicks::operations::hook_next_action_change_identity(78, 0x7810, 62, 1);
    assert!(runtime::dispatch_controller_named(78, "cloud", "main", "external_action").is_ok());
    assert_eq!(slight::gimmicks::operations::take_actions().len(), 1);
    assert_eq!(runtime::test_actor_slot_counts(), (1, 0, 1));
    assert_eq!(
        controller_count(78, "main"),
        0,
        "new identity cannot see accepted old-actor state"
    );
}

#[test]
fn match_clear_retains_compiled_configuration_but_resets_chart_state() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(76, 0x7600, 0);
    install_compiled(vec![("main", program(6, false, None, None))]);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 76);
    assert_eq!(controller_count(76, "main"), 7);
    runtime::clear_all();
    assert_eq!(
        runtime::test_effective_controller_source("cloud"),
        Some("compiled")
    );
    assert_eq!(runtime::test_actor_slot_counts(), (0, 0, 0));
    assert_eq!(controller_count(76, "main"), 6);
}

#[test]
fn compiled_resource_consumer_is_owner_routed_and_not_double_counted() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    let resource = model::ResourceSource::CloudLimitGroundedNeutral;
    install_compiled(vec![("main", program(0, false, Some(resource), None))]);
    assert!(runtime::resource_consumer());

    assert!(
        runtime::handle(&request(1, vec![program(0, false, None, None)], Vec::new(),)).accepted
    );
    assert!(
        !runtime::resource_consumer(),
        "live owner masks its baseline consumer"
    );

    runtime::clear_overrides();
    assert!(
        runtime::resource_consumer(),
        "live clear restores exactly the baseline consumer"
    );
}

#[test]
fn compiled_baseline_actor_pool_uses_shared_128_actor_capacity() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    install_compiled(vec![("main", program(0, false, None, None))]);
    for index in 0..128u32 {
        let boid = 30_000 + index;
        cloud(boid, 0x30_0000 + u64::from(index), (index % 8) as i32);
        runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, boid);
    }
    assert_eq!(runtime::test_actor_slot_counts(), (128, 0, 128));
    cloud(31_000, 0x31_0000, 0);
    slight::gimmicks::native_values::take_sample_count();
    runtime::dispatch(
        slight::gimmicks::dispatcher::GimmickHook::ChargeTick,
        31_000,
    );
    assert_eq!(runtime::test_actor_slot_counts(), (128, 0, 128));
    assert_eq!(slight::gimmicks::native_values::take_sample_count(), 0);
    runtime::clear_boid(30_000);
    runtime::dispatch(
        slight::gimmicks::dispatcher::GimmickHook::ChargeTick,
        31_000,
    );
    assert_eq!(runtime::test_actor_slot_counts(), (128, 0, 128));
}

#[test]
fn overlapping_hidden_baseline_and_live_actor_state_counts_one_boid() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(32_000, 0x32_0000, 0);
    install_compiled(vec![("main", program(1, false, None, None))]);
    let revision = install_controller(1, vec![("main", program(20, false, None, None))]);
    assert!(runtime::restart_controller_chart("cloud", 32_000, "main", revision).accepted);
    assert!(runtime::test_insert_hidden_baseline_state(32_000, "cloud"));
    assert_eq!(runtime::test_actor_slot_counts(), (1, 0, 1));
    runtime::clear_overrides();
    assert_eq!(runtime::test_actor_slot_counts(), (0, 0, 0));
    assert_eq!(
        runtime::test_effective_controller_source("cloud"),
        Some("compiled")
    );
}

fn named_add_program(initial: i32, delta: i32) -> model::LiveProgram {
    let mut authored = program(initial, false, None, None);
    let index = authored.transitions.len();
    authored.transitions.push(model::Transition {
        from: "active".into(),
        to: "active".into(),
        guard: model::Predicate::Compare {
            op: model::CmpOp::Eq,
            left: model::Expr::Const(model::Value::Int(1)),
            right: model::Expr::Const(model::Value::Int(1)),
        },
        on_frame: true,
        actions: vec![model::Action::Add {
            var: "count".into(),
            value: model::Expr::Const(model::Value::Int(delta)),
        }],
    });
    authored.event_bindings = vec![model::EventBinding {
        name: "external_action".into(),
        transitions: vec![index],
        fields: vec![],
    }];
    assert!(authored.validate().is_ok(), "{:?}", authored.validate());
    authored
}

fn named_typed_add_program(initial: i32) -> model::LiveProgram {
    let mut authored = named_add_program(initial, 0);
    authored.vars.extend([
        model::VarDef {
            keep_across_form_switch: false,
            name: "timer".into(),
            kind: model::VarKind::Timer,
            initial: model::Value::Int(5),
        },
        model::VarDef {
            keep_across_form_switch: false,
            name: "ratio_seen".into(),
            kind: model::VarKind::Float,
            initial: model::Value::Float(0.0),
        },
        model::VarDef {
            keep_across_form_switch: false,
            name: "enabled_seen".into(),
            kind: model::VarKind::Flag,
            initial: model::Value::Flag(false),
        },
    ]);
    let transition = authored.event_bindings[0].transitions[0];
    authored.event_bindings[0].fields = vec![
        model::EventField {
            name: "scale".into(),
            kind: model::VarKind::Float,
        },
        model::EventField {
            name: "amount".into(),
            kind: model::VarKind::Int,
        },
        model::EventField {
            name: "enabled".into(),
            kind: model::VarKind::Flag,
        },
    ];
    authored.transitions[transition].actions = vec![
        model::Action::Add {
            var: "count".into(),
            value: model::Expr::EventField {
                event: "external_action".into(),
                field: "amount".into(),
            },
        },
        model::Action::Set {
            var: "ratio_seen".into(),
            value: model::Expr::EventField {
                event: "external_action".into(),
                field: "scale".into(),
            },
        },
        model::Action::SetFlag {
            var: "enabled_seen".into(),
            value: model::Predicate::EventFlag {
                event: "external_action".into(),
                field: "enabled".into(),
            },
        },
    ];
    assert!(authored.validate().is_ok(), "{:?}", authored.validate());
    authored
}

fn named_native_article_program() -> model::LiveProgram {
    let mut authored = named_action_program(
        0,
        None,
        vec![model::Action::GenerateArticle {
            symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
        }],
    );
    authored.native_inputs = vec![NativeInput::CloudLimitGauge];
    let transition = authored.event_bindings[0].transitions[0];
    authored.transitions[transition].guard = model::Predicate::Compare {
        op: model::CmpOp::Gte,
        left: model::Expr::NativeFloat(NativeInput::CloudLimitGauge),
        right: model::Expr::Const(model::Value::Float(0.0)),
    };
    assert!(authored.validate().is_ok(), "{:?}", authored.validate());
    authored
}

fn baseline_two_effect_named_program() -> model::LiveProgram {
    let mut authored = named_action_program(
        0,
        None,
        vec![
            model::Action::Add {
                var: "count".into(),
                value: model::Expr::Const(model::Value::Int(1)),
            },
            model::Action::GenerateArticle {
                symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
            },
            model::Action::GenerateArticle {
                symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
            },
        ],
    );
    authored.native_inputs = vec![NativeInput::CloudLimitGauge];
    let transition = authored.event_bindings[0].transitions[0];
    authored.transitions[transition].guard = model::Predicate::Compare {
        op: model::CmpOp::Gte,
        left: model::Expr::NativeFloat(NativeInput::CloudLimitGauge),
        right: model::Expr::Const(model::Value::Float(0.0)),
    };
    assert!(authored.validate().is_ok(), "{:?}", authored.validate());
    authored
}

fn controller_row(boid: u32, chart_id: &str) -> ControllerStateRow {
    runtime::controller_wire_rows()
        .into_iter()
        .find(|row| row.actor.boid == boid && row.chart_id == chart_id)
        .unwrap_or_else(|| panic!("missing controller row for {boid}/{chart_id}"))
}

fn controller_count(boid: u32, chart_id: &str) -> i32 {
    match controller_row(boid, chart_id).actor.vars["count"] {
        model::Value::Int(value) => value,
        other => panic!("unexpected count value {other:?}"),
    }
}

fn controller_timer(boid: u32, chart_id: &str) -> i32 {
    match controller_row(boid, chart_id).actor.vars["timer"] {
        model::Value::Int(value) => value,
        other => panic!("unexpected timer value {other:?}"),
    }
}

#[test]
fn v5_named_event_mutates_only_its_explicit_chart() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(70, 0x7000, 0);
    let revision = install_controller(
        1,
        vec![
            ("left", named_add_program(3, 4)),
            ("right", named_add_program(9, 7)),
        ],
    );

    runtime::dispatch_controller_named(70, "cloud", "right", "external_action").unwrap();
    assert_eq!(controller_count(70, "left"), 3);
    assert_eq!(controller_count(70, "right"), 16);
    assert_eq!(runtime::revision(), revision);
}

#[test]
fn v5_frame_steps_independent_timers_and_causal_expiry_per_chart() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(71, 0x7100, 0);
    let short = ordinary_expiry_action_program(0, Vec::new());
    let mut long = short.clone();
    long.vars
        .iter_mut()
        .find(|var| var.name == "timer")
        .unwrap()
        .initial = model::Value::Int(3);
    install_controller(1, vec![("short", short), ("long", long)]);

    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 71);
    assert_eq!(controller_timer(71, "short"), 0);
    assert_eq!(controller_timer(71, "long"), 2);
    assert_eq!(controller_count(71, "short"), 101);
    assert_eq!(controller_count(71, "long"), 1);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 71);
    assert_eq!(controller_count(71, "short"), 102);
    assert_eq!(controller_count(71, "long"), 2);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 71);
    assert_eq!(controller_count(71, "short"), 103);
    assert_eq!(controller_count(71, "long"), 103);
}

#[test]
fn v5_resource_dispatch_is_exact_and_generic_timeout_does_not_broadcast() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(90, 0x9000, 0);
    install_controller(
        1,
        vec![
            ("main", program(0, false, None, Some(1))),
            (
                "resource",
                program(
                    5,
                    false,
                    Some(model::ResourceSource::CloudLimitIdleExpiry),
                    None,
                ),
            ),
            (
                "other_resource",
                program(
                    20,
                    false,
                    Some(model::ResourceSource::CloudLimitIdleExpiry),
                    None,
                ),
            ),
        ],
    );
    assert!(runtime::resource_consumer());
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::Timeout, 90);
    assert_eq!(controller_count(90, "main"), 0);
    assert_eq!(controller_count(90, "resource"), 5);
    assert_eq!(controller_count(90, "other_resource"), 20);
    assert_eq!(slight::gimmicks::native_values::take_sample_count(), 0);
    runtime::dispatch(
        slight::gimmicks::dispatcher::GimmickHook::CloudLimitIdleExpiry,
        90,
    );
    assert_eq!(controller_count(90, "resource"), 15);
    assert_eq!(controller_count(90, "other_resource"), 30);
    assert_eq!(controller_count(90, "main"), 0);
    assert_eq!(controller_timer(90, "main"), 1);
}

#[test]
fn v5_status_dispatch_selects_only_registered_charts_without_ticking_siblings() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(91, 0x9100, 0);
    install_controller(
        1,
        vec![
            ("main", program(0, false, None, Some(3))),
            ("status", program(5, true, None, None)),
        ],
    );
    runtime::dispatch_status(0x1ee, 91);
    assert!(slight::gimmicks::operations::take_actions().is_empty());
    runtime::dispatch_status(0x1ef, 91);
    assert_eq!(slight::gimmicks::operations::take_actions().len(), 1);
    assert_eq!(controller_timer(91, "main"), 3);
    assert_eq!(controller_count(91, "main"), 0);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::Spend, 91);
    assert!(slight::gimmicks::operations::take_actions().is_empty());
}

#[test]
fn v5_failed_union_input_snapshot_commits_no_chart_or_reservation() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(92, 0x9200, 0);
    install_controller(
        1,
        vec![
            ("main", program(0, false, None, Some(3))),
            ("native", named_native_article_program()),
        ],
    );
    slight::gimmicks::native_values::set_test_sample(None);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 92);
    assert_eq!(controller_count(92, "main"), 0);
    assert_eq!(controller_timer(92, "main"), 3);
    assert_eq!(controller_count(92, "native"), 0);
    assert_eq!(runtime::test_actor_slot_counts(), (0, 0, 0));
    assert_eq!(slight::gimmicks::native_values::take_sample_count(), 1);
    assert!(slight::gimmicks::operations::take_actions().is_empty());
}

#[test]
fn v5_failed_expiry_effect_does_not_replay_the_expiry_next_frame() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(93, 0x9300, 0);
    let authored = ordinary_expiry_action_program(
        0,
        vec![model::Action::GenerateArticle {
            symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
        }],
    );
    install_controller(1, vec![("main", authored)]);
    slight::gimmicks::operations::fail_action_at(1);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 93);
    assert_eq!(controller_count(93, "main"), 101);
    assert_eq!(controller_timer(93, "main"), 0);
    assert_eq!(slight::gimmicks::operations::take_actions().len(), 1);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 93);
    assert_eq!(controller_count(93, "main"), 102);
    assert!(slight::gimmicks::operations::take_actions().is_empty());
}

#[test]
fn v5_actor_cap_counts_pending_compiled_dispatch_in_the_same_union() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    let revision = install_controller(1, vec![("main", named_native_article_program())]);
    fighter(94, 0x9400, 1, "samus");
    let identity = runtime::Identity::from_live_fighter(94, "samus").unwrap();
    let reservation = runtime::begin_compiled_dispatch(94, "samus", identity).unwrap();
    for index in 0..127u32 {
        let boid = 30_000 + index;
        cloud(boid, 0x30_0000 + u64::from(index), (index % 8) as i32);
        assert!(runtime::restart_controller_chart("cloud", boid, "main", revision).accepted);
    }
    assert_eq!(runtime::test_actor_slot_counts(), (127, 1, 128));
    cloud(31_000, 0x31_0000, 0);
    assert!(!runtime::restart_controller_chart("cloud", 31_000, "main", revision).accepted);
    runtime::dispatch(
        slight::gimmicks::dispatcher::GimmickHook::ChargeTick,
        31_000,
    );
    assert!(
        runtime::dispatch_controller_named(31_000, "cloud", "main", "external_action").is_err()
    );
    assert_eq!(slight::gimmicks::native_values::take_sample_count(), 0);
    assert!(slight::gimmicks::operations::take_actions().is_empty());
    drop(reservation);
    assert!(runtime::restart_controller_chart("cloud", 31_000, "main", revision).accepted);
    assert_eq!(runtime::test_actor_slot_counts(), (128, 0, 128));
}

#[test]
fn v5_named_sampling_identity_reuse_cancels_effects_and_state_commit() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(95, 0x9500, 0);
    install_controller(1, vec![("main", named_native_article_program())]);
    slight::gimmicks::native_values::recycle_actor_after_sample();
    assert!(runtime::dispatch_controller_named(95, "cloud", "main", "external_action").is_err());
    assert!(slight::gimmicks::operations::take_actions().is_empty());
    assert_eq!(runtime::test_actor_slot_counts(), (0, 0, 0));
}

#[test]
fn v5_install_invalidates_matching_compiled_ticket_and_empty_snapshot_restores_baseline() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(96, 0x9600, 0);
    let identity = runtime::Identity::from_live_fighter(96, "cloud").unwrap();
    let reservation = runtime::begin_compiled_dispatch(96, "cloud", identity).unwrap();
    install_controller(1, vec![("main", program(0, false, None, None))]);
    assert!(!runtime::compiled_dispatch_ticket_is_current(
        identity,
        &reservation
    ));
    assert!(runtime::suppress_compiled(96, "cloud"));
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 96);
    assert_eq!(controller_count(96, "main"), 1);
    install_controller(2, Vec::new());
    assert!(!runtime::overrides("cloud"));
    assert!(!runtime::suppress_compiled(96, "cloud"));
    assert!(runtime::controller_wire_rows().is_empty());
    assert_eq!(runtime::test_actor_slot_counts(), (0, 0, 0));
    assert!(runtime::dispatch_named(96, "cloud", "external_action").is_none());
}

#[test]
fn v5_stock_start_resets_all_charts_and_targeted_restart_resets_one() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(72, 0x7200, 0);
    let revision = install_controller(
        1,
        vec![
            ("main", program(5, false, None, Some(5))),
            ("extra", program(12, false, None, Some(7))),
        ],
    );
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 72);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 72);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::StockStart, 72);
    assert_eq!(controller_count(72, "main"), 5);
    assert_eq!(controller_timer(72, "main"), 5);
    assert_eq!(controller_count(72, "extra"), 12);
    assert_eq!(controller_timer(72, "extra"), 7);

    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 72);
    assert!(runtime::restart_controller_chart("cloud", 72, "main", revision).accepted);
    assert_eq!(controller_count(72, "main"), 5);
    assert_eq!(controller_timer(72, "main"), 5);
    assert_eq!(controller_count(72, "extra"), 13);
    assert_eq!(controller_timer(72, "extra"), 6);
}

#[test]
fn v5_replacement_reconciles_all_actors_by_exact_chart_identity() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(74, 0x7400, 0);
    cloud(75, 0x7500, 1);
    install_controller(
        1,
        vec![
            ("keep", program(0, false, None, Some(5))),
            ("change", program(10, false, None, Some(5))),
            ("remove", program(20, false, None, Some(5))),
        ],
    );
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 74);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 74);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 75);

    install_controller(
        2,
        vec![
            ("keep", program(0, false, None, Some(5))),
            ("change", program(90, false, None, Some(9))),
            ("added", program(30, false, None, Some(2))),
        ],
    );
    assert_eq!(controller_count(74, "keep"), 2);
    assert_eq!(controller_count(75, "keep"), 1);
    assert_eq!(controller_timer(74, "keep"), 3);
    assert_eq!(controller_count(74, "change"), 90);
    assert_eq!(controller_timer(74, "change"), 9);
    assert_eq!(controller_count(74, "added"), 30);
    assert_eq!(controller_timer(74, "added"), 2);
    assert!(!runtime::controller_wire_rows()
        .iter()
        .any(|row| row.actor.boid == 74 && row.chart_id == "remove"));
    // Both actors must be bound to the same replacement coordinator.
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 74);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 75);
    assert_eq!(controller_count(74, "keep"), 3);
    assert_eq!(controller_count(75, "keep"), 2);
    assert_eq!(controller_count(74, "change"), 91);
    assert_eq!(controller_count(75, "change"), 91);
}

#[test]
fn invalid_v5_candidate_keeps_the_active_complete_snapshot() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(77, 0x7700, 0);
    let active = controller_bundle(1, vec![("main", program(4, false, None, None))]);
    let accepted = runtime::handle(&active);
    assert!(accepted.accepted, "{}", accepted.message);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 77);
    assert_eq!(controller_count(77, "main"), 5);

    let mut malformed: serde_json::Value = serde_json::from_str(&active).unwrap();
    malformed["gimmick_behavior"]["revision"] = 2.into();
    malformed["gimmick_behavior"]["charts"][0]["chart_id"] = " ".into();
    let malformed = malformed.to_string();
    let rejected = runtime::handle(&malformed);
    assert!(!rejected.accepted);
    assert_eq!(runtime::revision(), 1);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 77);
    assert_eq!(controller_count(77, "main"), 6);
}

#[test]
fn v5_stale_revision_and_same_revision_conflicts_do_not_replace_content() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(78, 0x7800, 0);
    let active = controller_bundle(5, vec![("main", program(1, false, None, None))]);
    assert!(runtime::handle(&active).accepted);
    assert!(runtime::handle(&active).accepted);

    let conflict = controller_bundle(5, vec![("main", program(8, false, None, None))]);
    let conflict_ack = runtime::handle(&conflict);
    assert!(!conflict_ack.accepted);
    let stale = controller_bundle(4, vec![("main", program(0, false, None, None))]);
    assert!(!runtime::handle(&stale).accepted);
    assert_eq!(runtime::revision(), 5);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 78);
    assert_eq!(controller_count(78, "main"), 2);
}

#[test]
fn v5_clear_and_identity_reuse_start_fresh_actor_state() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(79, 0x7900, 0);
    install_controller(1, vec![("main", program(0, false, None, None))]);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 79);
    assert_eq!(controller_count(79, "main"), 1);

    runtime::clear_boid(79);
    assert_eq!(controller_count(79, "main"), 0);
    cloud(79, 0x7910, 1);
    assert_eq!(controller_count(79, "main"), 0);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 79);
    assert_eq!(controller_count(79, "main"), 1);

    cloud(79, 0x7920, 2);
    assert_eq!(controller_count(79, "main"), 0);
    runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 79);
    assert_eq!(controller_count(79, "main"), 1);
    runtime::clear_all();
    assert_eq!(controller_count(79, "main"), 0);
    assert_eq!(runtime::test_actor_slot_counts().2, 0);
}

#[test]
fn v5_accepted_named_delivery_replacement_or_teardown_cancels_remaining_effects() {
    let _serial = TEST_LOCK.lock().unwrap();
    for teardown in [false, true] {
        reset();
        cloud(80, 0x8000, 0);
        let mut authored = named_action_program(
            0,
            None,
            vec![model::Action::Add {
                var: "count".into(),
                value: model::Expr::Const(model::Value::Int(1)),
            }],
        );
        authored.transitions[authored.event_bindings[0].transitions[0]]
            .actions
            .extend((0..2).map(|_| model::Action::GenerateArticle {
                symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
            }));
        authored.id = "v5:main".into();
        install_controller(1, vec![("main", authored)]);
        if teardown {
            slight::gimmicks::operations::hook_next_action_clear_boid(80);
        } else {
            let replacement = controller_bundle(2, vec![("main", program(40, false, None, None))]);
            slight::gimmicks::operations::hook_next_action_replace_snapshot(replacement);
        }
        assert!(runtime::dispatch_controller_named(80, "cloud", "main", "external_action").is_ok());
        assert_eq!(controller_count(80, "main"), if teardown { 0 } else { 40 });
        assert_eq!(slight::gimmicks::operations::take_actions().len(), 1);
        assert_eq!(runtime::test_actor_slot_counts().1, 0);
    }
}

#[test]
fn v5_actor_capacity_refuses_before_native_sampling_or_effects() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    let revision = install_controller(1, vec![("main", named_native_article_program())]);
    for index in 0..128u32 {
        let boid = 20_000 + index;
        cloud(boid, 0x20_0000 + u64::from(index), (index % 8) as i32);
        let ack = runtime::restart_controller_chart("cloud", boid, "main", revision);
        assert!(ack.accepted, "actor {boid}: {}", ack.message);
    }
    cloud(21_000, 0x21_0000, 0);
    slight::gimmicks::native_values::take_sample_count();

    let error = runtime::dispatch_controller_named(21_000, "cloud", "main", "external_action");
    assert!(error.unwrap_err().contains("capacity"));
    runtime::dispatch(
        slight::gimmicks::dispatcher::GimmickHook::ChargeTick,
        21_000,
    );
    let restart = runtime::restart_controller_chart("cloud", 21_000, "main", revision);
    assert!(!restart.accepted && restart.message.contains("capacity"));
    assert_eq!(slight::gimmicks::native_values::take_sample_count(), 0);
    assert!(slight::gimmicks::operations::take_actions().is_empty());
    assert_eq!(runtime::test_actor_slot_counts(), (128, 0, 128));

    runtime::clear_boid(20_000);
    assert!(runtime::restart_controller_chart("cloud", 21_000, "main", revision).accepted);
    assert_eq!(runtime::test_actor_slot_counts(), (128, 0, 128));
    runtime::clear_all();
    assert!(runtime::restart_controller_chart("cloud", 20_000, "main", revision).accepted);
    assert_eq!(runtime::test_actor_slot_counts(), (1, 0, 1));
}

#[test]
fn main_named_root_delivers_nested_typed_actions_without_ticking_siblings_or_falling_back() {
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(108, 0x10800, 0);
    let mut authored = named_typed_add_program(0);
    let receiver = authored.event_bindings[0].transitions[0];
    let article = "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE";
    authored.transitions[receiver].actions.extend([
        model::Action::GenerateArticle {
            symbol: article.into(),
        },
        model::Action::RemoveArticle {
            symbol: article.into(),
        },
        model::Action::GenerateArticle {
            symbol: article.into(),
        },
    ]);
    let root = authored.transitions.len();
    let mut transition = authored.transitions[receiver].clone();
    transition.actions = vec![model::Action::EmitEventArguments {
        name: "external_action".into(),
        arguments: [
            ("scale", model::Value::Float(1.5)),
            ("amount", model::Value::Int(7)),
            ("enabled", model::Value::Flag(true)),
        ]
        .map(|(field, value)| model::EventArgument {
            field: field.into(),
            value,
        })
        .to_vec(),
    }];
    authored.transitions.push(transition);
    authored.event_bindings.push(model::EventBinding {
        name: "start".into(),
        transitions: vec![root],
        fields: vec![],
    });
    install_controller(
        1,
        vec![("main", authored), ("sibling", named_add_program(10, 1))],
    );
    runtime::dispatch_named(108, "cloud", "start")
        .unwrap()
        .unwrap();
    assert_eq!(controller_count(108, "main"), 7);
    assert_eq!(controller_timer(108, "main"), 5);
    assert_eq!(controller_count(108, "sibling"), 10);
    assert_eq!(
        runtime::read_controller_value(108, "cloud", "main", "ratio_seen"),
        Some(model::Value::Float(1.5))
    );
    assert_eq!(
        runtime::read_controller_value(108, "cloud", "main", "enabled_seen"),
        Some(model::Value::Flag(true))
    );
    use slight::gimmicks::operations::ObservedAction::{Generate, Remove};
    assert_eq!(
        slight::gimmicks::operations::take_actions(),
        [
            Generate(108, article.into()),
            Remove(108, article.into()),
            Generate(108, article.into())
        ]
    );
    assert!(matches!(
        runtime::dispatch_named(108, "cloud", "missing"),
        Some(Err(_))
    ));
    assert_eq!(controller_count(108, "main"), 7);
    slight::agents::clear();
    assert!(matches!(
        runtime::dispatch_named(108, "cloud", "start"),
        Some(Err(_))
    ));
    assert!(runtime::controller_wire_rows().is_empty());
    assert!(slight::gimmicks::operations::take_actions().is_empty());
}

#[test]
fn phase_reads_and_commands_deliver_on_both_controller_sources_and_reject_missing_sample() {
    use slight::gimmicks::{
        native_inputs::{PhaseInput, SituationKind},
        phase_commands::Command,
        phase_control,
    };
    let _serial = TEST_LOCK.lock().unwrap();
    for compiled in [false, true] {
        reset();
        cloud(180, 0x18000, 0);
        let mut authored = program(0, false, None, None);
        authored.transitions[0].guard = model::Predicate::And(
            Box::new(model::Predicate::InStatus {
                symbol: "FIGHTER_STATUS_KIND_SPECIAL_N".into(),
            }),
            Box::new(model::Predicate::Situation {
                kind: SituationKind::Ground,
            }),
        );
        authored.transitions[0].actions = vec![
            model::Action::Set {
                var: "count".into(),
                value: model::Expr::StatusFrame,
            },
            model::Action::SetMotionRate {
                rate: model::Expr::MotionFrame,
            },
            model::Action::AddSpeed {
                x: model::Expr::MotionFrame,
                y: model::Expr::Const(model::Value::Float(-0.5)),
            },
        ];
        if compiled {
            install_compiled(vec![("main", authored)]);
        } else {
            install_controller(1, vec![("main", authored)]);
        }
        let dispatch =
            || runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 180);
        dispatch();
        assert_eq!(
            runtime::read_controller_value(180, "cloud", "main", "count"),
            Some(model::Value::Int(0))
        );
        assert!(phase_control::take_commands().is_empty());
        let mut phase = PhaseInput {
            fighter: "cloud".into(),
            status: slight::gimmicks::registry::resolve(
                "cloud",
                "FIGHTER_STATUS_KIND_SPECIAL_N",
                slight::gimmicks::registry::IdentifierKind::FighterStatus,
            )
            .unwrap()
            .value as i32,
            status_frame: 17,
            motion_frame: 1.5,
            motion: 0,
            situation: Some(SituationKind::Ground),
        };
        phase_control::set_sample(Some(phase.clone()));
        dispatch();
        assert_eq!(
            runtime::read_controller_value(180, "cloud", "main", "count"),
            Some(model::Value::Int(17))
        );
        assert_eq!(
            phase_control::take_commands(),
            vec![Command::Rate(1.5), Command::Speed { x: -1.5, y: -0.5 }]
        );
        phase.situation = Some(SituationKind::Air);
        phase_control::set_sample(Some(phase.clone()));
        dispatch();
        assert!(phase_control::take_commands().is_empty());
        phase.situation = Some(SituationKind::Ground);
        phase.motion_frame = 21.0;
        phase.status_frame = 30;
        phase_control::set_sample(Some(phase));
        dispatch();
        assert!(phase_control::take_commands().is_empty());
        assert_eq!(
            runtime::read_controller_value(180, "cloud", "main", "count"),
            Some(model::Value::Int(17))
        );
    }
}

#[test]
fn status_request_is_issued_once_per_pressed_edge() {
    use slight::gimmicks::native_inputs::{ButtonEdge, ControllerInput, InputButton};
    let _serial = TEST_LOCK.lock().unwrap();
    for compiled in [false, true] {
        reset();
        cloud(181, 0x18100, 0);
        let mut authored = program(0, false, None, None);
        authored.transitions[0].guard = model::Predicate::Button {
            button: InputButton::Jump,
            edge: ButtonEdge::Pressed,
        };
        authored.transitions[0].actions = vec![model::Action::RequestFighterStatus {
            symbol: "FIGHTER_STATUS_KIND_SPECIAL_HI".into(),
        }];
        if compiled {
            install_compiled(vec![("main", authored)]);
        } else {
            install_controller(1, vec![("main", authored)]);
        }
        let mut input = ControllerInput {
            facing: 1.0,
            ..Default::default()
        };
        input
            .buttons
            .insert((InputButton::Jump, ButtonEdge::Pressed), true);
        input
            .buttons
            .insert((InputButton::Jump, ButtonEdge::Held), true);
        slight::gimmicks::native_values::set_controller_input(input.clone());
        // A status callback can sample the same native trigger more than once.
        slight::gimmicks::operations::hook_next_action_dispatch_charge_tick(181);
        for _ in 0..3 {
            runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 181);
        }
        assert_eq!(slight::gimmicks::operations::take_actions().len(), 1);
        input
            .buttons
            .insert((InputButton::Jump, ButtonEdge::Pressed), false);
        input
            .buttons
            .insert((InputButton::Jump, ButtonEdge::Held), false);
        slight::gimmicks::native_values::set_controller_input(input.clone());
        runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 181);
        assert!(slight::gimmicks::operations::take_actions().is_empty());
        input
            .buttons
            .insert((InputButton::Jump, ButtonEdge::Pressed), true);
        input
            .buttons
            .insert((InputButton::Jump, ButtonEdge::Held), true);
        slight::gimmicks::native_values::set_controller_input(input);
        runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 181);
        assert_eq!(slight::gimmicks::operations::take_actions().len(), 1);
    }
}

#[test]
fn rejected_status_dispatch_commits_no_operations() {
    let _serial = TEST_LOCK.lock().unwrap();
    for compiled in [false, true] {
        for root in ["named", "values", "frame"] {
            reset();
            cloud(0xF0AA_0009, 0x18200, 0);
            let actions = vec![
                model::Action::Add {
                    var: "count".into(),
                    value: model::Expr::Const(model::Value::Int(1)),
                },
                model::Action::RequestFighterStatus {
                    symbol: "FIGHTER_STATUS_KIND_SPECIAL_HI".into(),
                },
                model::Action::GenerateArticle {
                    symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
                },
            ];
            let authored = if root == "frame" {
                ordinary_action_program(0, actions)
            } else {
                named_action_program(0, None, actions)
            };
            if compiled {
                install_compiled(vec![("main", authored)]);
            } else {
                install_controller(1, vec![("main", authored)]);
            }
            let before = runtime::controller_wire_rows();
            match root {
                "named" => assert!(runtime::dispatch_controller_named(
                    0xF0AA_0009,
                    "cloud",
                    "main",
                    "external_action"
                )
                .is_err()),
                "values" => assert!(runtime::dispatch_controller_named_values(
                    0xF0AA_0009,
                    "cloud",
                    "main",
                    "external_action",
                    &[]
                )
                .is_err()),
                _ => {
                    runtime::dispatch(
                        slight::gimmicks::dispatcher::GimmickHook::ChargeTick,
                        0xF0AA_0009,
                    );
                    assert!(slight::diag::take_notes()
                        .iter()
                        .any(|note| note.contains("injected native article generation rejection")));
                }
            }
            assert!(slight::gimmicks::operations::take_actions().is_empty());
            assert_eq!(runtime::controller_wire_rows(), before);
            assert_eq!(runtime::test_actor_slot_counts(), (0, 0, 0));
        }
    }
}

#[test]
fn damage_modifiers_route_from_live_and_compiled_controllers_without_phase_input() {
    use slight::gimmicks::{
        native_inputs::{CombatChannel, CombatOp, PolicyScope},
        phase_commands::Command,
        phase_control,
    };
    let policy = |scope, op| model::Action::CombatPolicy { scope, op };
    let _serial = TEST_LOCK.lock().unwrap();
    for compiled in [false, true] {
        reset();
        cloud(180, 0x18000, 0);
        let mut authored = program(0, false, None, None);
        let float = |v| model::Expr::Const(model::Value::Float(v));
        authored.transitions[0].actions = vec![
            model::Action::AddDamage {
                amount: float(-5.0),
            },
            policy(PolicyScope::Status, CombatOp::DamageTaken(float(0.8))),
            policy(PolicyScope::Status, CombatOp::KnockbackTaken(float(1.2))),
            policy(PolicyScope::Status, CombatOp::Armor(float(12.0))),
            policy(PolicyScope::Status, CombatOp::SuperArmor),
            policy(PolicyScope::Frames(30), CombatOp::Intangible),
            policy(PolicyScope::UntilCleared, CombatOp::DamageDealt(float(1.5))),
            policy(PolicyScope::Status, CombatOp::Clear(CombatChannel::Armor)),
        ];
        if compiled {
            install_compiled(vec![("main", authored)]);
        } else {
            install_controller(1, vec![("main", authored)]);
        }
        runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 180);
        assert_eq!(
            phase_control::take_commands(),
            vec![
                Command::AddDamage(-5.0),
                Command::Combat(PolicyScope::Status, CombatOp::DamageTaken(0.8)),
                Command::Combat(PolicyScope::Status, CombatOp::KnockbackTaken(1.2)),
                Command::Combat(PolicyScope::Status, CombatOp::Armor(12.0)),
                Command::Combat(PolicyScope::Status, CombatOp::SuperArmor),
                Command::Combat(PolicyScope::Frames(30), CombatOp::Intangible),
                Command::Combat(PolicyScope::UntilCleared, CombatOp::DamageDealt(1.5)),
                Command::Combat(PolicyScope::Status, CombatOp::Clear(CombatChannel::Armor)),
            ]
        );
    }
}

#[test]
fn grab_and_throw_events_route_live_and_compiled_with_fresh_motion_guards() {
    use slight::gimmicks::{
        dispatcher::GimmickHook,
        native_inputs::{hash40, PhaseInput, SituationKind},
        phase_control,
    };
    let _serial = TEST_LOCK.lock().unwrap();
    for compiled in [false, true] {
        for (native, hook) in [
            (GimmickHook::Grab, model::HookName::OnGrab),
            (GimmickHook::Throw, model::HookName::OnThrow),
            (GimmickHook::Grabbed, model::HookName::OnGrabbed),
        ] {
            reset();
            cloud(180, 0x18000, 0);
            let mut authored = program(0, false, None, None);
            authored.bindings[0].hook = hook;
            authored.transitions[0].guard = model::Predicate::InMotion {
                motion: "throw_f".into(),
            };
            if compiled {
                install_compiled(vec![("main", authored)]);
            } else {
                install_controller(1, vec![("main", authored)]);
            }
            let mut phase = PhaseInput {
                fighter: "cloud".into(),
                status: 0,
                status_frame: 4,
                motion_frame: 4.0,
                motion: hash40("throw_b"),
                situation: Some(SituationKind::Ground),
            };
            phase_control::set_sample(Some(phase.clone()));
            runtime::dispatch(native, 180);
            assert_eq!(
                runtime::read_controller_value(180, "cloud", "main", "count"),
                Some(model::Value::Int(0))
            );
            phase.motion = hash40("throw_f");
            phase_control::set_sample(Some(phase));
            runtime::dispatch(GimmickHook::ChargeTick, 180);
            assert_eq!(
                runtime::read_controller_value(180, "cloud", "main", "count"),
                Some(model::Value::Int(0))
            );
            runtime::dispatch(native, 180);
            assert_eq!(
                runtime::read_controller_value(180, "cloud", "main", "count"),
                Some(model::Value::Int(1))
            );
        }
    }
}

#[test]
fn movement_dispatch_has_static_live_parity_and_rejects_stale_samples() {
    use slight::gimmicks::{
        native_inputs::MovementAction as M, phase_commands::Command, phase_control,
    };
    let _serial = TEST_LOCK.lock().unwrap();
    for compiled in [false, true] {
        reset();
        cloud(184, 0x18400, 0);
        let mut authored = program(0, false, None, None);
        authored.transitions[0].guard = model::Predicate::Compare {
            op: model::CmpOp::Lt,
            left: model::Expr::NativeFloat(NativeInput::Fighter {
                value: FighterValue::SpeedY,
                fighter: "cloud".into(),
            }),
            right: model::Expr::Const(model::Value::Float(1.0)),
        };
        let operations = [
            M::SetGravity { enabled: false },
            M::SetSpeed {
                x: None,
                y: Some(0.0),
            },
            M::MoveBy { dx: 30.0, dy: 0.0 },
            M::SetFacing(slight::gimmicks::native_inputs::Facing::Turn),
        ];
        authored.transitions[0].actions = operations
            .iter()
            .copied()
            .map(model::Action::Movement)
            .collect();
        authored.native_inputs = vec![NativeInput::Fighter {
            value: FighterValue::SpeedY,
            fighter: "cloud".into(),
        }];
        if compiled {
            install_compiled(vec![("main", authored)]);
        } else {
            install_controller(1, vec![("main", authored)]);
        }
        let dispatch =
            || runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 184);
        slight::gimmicks::native_values::set_test_sample(Some(-2.0));
        phase_control::take_commands();
        dispatch();
        assert_eq!(
            phase_control::take_commands(),
            operations.map(Command::Movement)
        );
        slight::gimmicks::native_values::set_test_sample(Some(2.0));
        dispatch();
        assert!(phase_control::take_commands().is_empty());
        slight::gimmicks::native_values::set_test_sample(None);
        dispatch();
        assert!(phase_control::take_commands().is_empty());
        slight::gimmicks::native_values::set_test_sample(Some(-2.0));
        slight::gimmicks::native_values::recycle_actor_after_sample();
        dispatch();
        assert!(phase_control::take_commands().is_empty());
    }
}

#[test]
fn incoming_veto_is_synchronous_for_live_and_compiled_controllers() {
    let _serial = TEST_LOCK.lock().unwrap();
    for compiled in [false, true] {
        reset();
        cloud(120, 0x12000, 0);
        let mut authored = ordinary_action_program(
            0,
            vec![
                model::Action::CancelIncomingHit,
                model::Action::Add {
                    var: "count".into(),
                    value: model::Expr::Const(model::Value::Int(1)),
                },
            ],
        );
        authored.transitions.truncate(1);
        authored.bindings = vec![model::HookBinding {
            hook: model::HookName::OnIncomingHit,
            article: None,
            transitions: vec![0],
        }];
        if compiled {
            install_compiled(vec![("main", authored)]);
        } else {
            install_controller(1, vec![("main", authored)]);
        }
        let input = slight::gimmicks::native_inputs::DamageInput::new(
            slight::gimmicks::native_inputs::DamageDirection::Incoming,
            12.0,
        )
        .unwrap();
        assert!(!runtime::dispatch_incoming(120, input, || false));
        assert_eq!(
            runtime::read_controller_value(120, "cloud", "main", "count"),
            Some(model::Value::Int(0))
        );
        assert!(runtime::dispatch_incoming(120, input, || true));
        assert_eq!(
            runtime::read_controller_value(120, "cloud", "main", "count"),
            Some(model::Value::Int(1))
        );
        // Cancellation must never escape into the deferred native-action queue.
        assert!(slight::gimmicks::operations::take_actions().is_empty());
    }
}

#[test]
fn owned_kinetics_freeze_under_stop_and_deduplicate_the_same_entry_frame() {
    use slight::gimmicks::{
        native_inputs::{NativeSnapshot, PhaseInput, SituationKind},
        status_model::Phase,
    };
    let _serial = TEST_LOCK.lock().unwrap();
    reset();
    cloud(106, 0x10600, 0);
    let program:model::LiveProgram=serde_json::from_value(serde_json::json!({
        "version":model::MODEL_VERSION,"id":"kinetics","fighter":"cloud","initial":"idle","states":["idle","cape"],"vars":[],"bindings":[],
        "status_bindings":[{"state":"cape","binding":{"target":"FIGHTER_STATUS_KIND_SPECIAL_S","policy":"Owned","roots":{"Pre":[0],"MainEnter":[1],"MainLoop":[2]}}}],
        "transitions":[
            {"from":"cape","to":"cape","on_frame":false,"guard":{"Compare":{"op":"eq","left":{"Const":{"Int":1}},"right":{"Const":{"Int":1}}}},"actions":[{"Status":{"InitializeStatus":{}}}]},
            {"from":"cape","to":"cape","on_frame":false,"guard":{"Compare":{"op":"eq","left":{"Const":{"Int":1}},"right":{"Const":{"Int":1}}}},"actions":[{"Status":{"PlayMotion":{"motion":"special_s","frame":{"Start":{"Const":{"Float":0.0}}},"rate":{"Const":{"Float":1.0}}}}}]},
            {"from": "cape", "to": "cape", "on_frame": false, "guard": "SituationChanged", "actions": [{"Status": {"ConfigureEnergy": {"energy": "Gravity", "op": {"Speed": {"x": {"Const": {"Float": 0.0}}, "y": {"Const": {"Float": 1.0}}}}}}}]}
        ]
    })).unwrap();
    install_compiled(vec![("main", program)]);
    let status = slight::gimmicks::registry::resolve(
        "cloud",
        "FIGHTER_STATUS_KIND_SPECIAL_S",
        slight::gimmicks::registry::IdentifierKind::FighterStatus,
    )
    .unwrap()
    .value as i32;
    let mut snapshot = NativeSnapshot::default();
    snapshot.phase = Some(PhaseInput {
        fighter: "cloud".into(),
        status,
        status_frame: 1,
        motion_frame: 12.0,
        motion: 1,
        situation: Some(SituationKind::Air),
    });
    snapshot.situation_changed = Some(true);
    runtime::status_inputs(106, "cloud", status, Phase::Pre)
        .unwrap()
        .unwrap();
    for phase in [Phase::Pre, Phase::MainEnter] {
        let _ = runtime::status_phase(
            106,
            "cloud",
            status,
            phase,
            &snapshot,
            |_| Ok(()),
            |_| Ok(false),
        )
        .unwrap();
    }
    snapshot.stopped = true;
    let delivered = std::cell::Cell::new(0);
    let tick = |snapshot: &NativeSnapshot| {
        runtime::status_phase(
            106,
            "cloud",
            status,
            Phase::MainLoop,
            snapshot,
            |_| Ok(()),
            |_| {
                delivered.set(delivered.get() + 1);
                Ok(false)
            },
        )
        .unwrap()
    };
    assert_eq!(tick(&snapshot), Some(false));
    assert_eq!(delivered.get(), 0);
    snapshot.stopped = false;
    assert_eq!(tick(&snapshot), Some(false));
    assert_eq!(tick(&snapshot), Some(false));
    assert_eq!(delivered.get(), 1);
    snapshot.phase.as_mut().unwrap().status_frame = 2;
    snapshot.situation_changed = Some(false);
    assert_eq!(tick(&snapshot), Some(false));
    assert_eq!(delivered.get(), 1);
    snapshot.phase.as_mut().unwrap().status_frame = 3;
    snapshot.situation_changed = Some(true);
    assert_eq!(tick(&snapshot), Some(false));
    assert_eq!(delivered.get(), 2);
}

#[test]
fn common_check_handled_stops_later_rules_and_preserves_entry_revision() {
    use slight::gimmicks::{
        native_inputs::{NativeSnapshot, PhaseInput},
        status_commands::{Command, CommonTransition},
        status_model::Phase,
    };
    let _serial = TEST_LOCK.lock().unwrap();
    for handled in [true, false] {
        reset();
        cloud(106, 0x10600, 0);
        let mut program:model::LiveProgram=serde_json::from_value(serde_json::json!({
            "version":model::MODEL_VERSION,"id":"cancel","fighter":"cloud","initial":"idle","states":["idle","owned"],"vars":[{"name":"count","kind":"int","initial":{"Int":0}}],"bindings":[],
            "custom_statuses":{"guard":0},
            "status_bindings":[{"state":"owned","binding":{"target":{"key":"guard","slot":0},"policy":"Owned","roots":{"Pre":[0],"MainEnter":[1],"MainLoop":[2,3]}}}],
            "transitions":[
                {"from":"owned","to":"owned","on_frame":false,"guard":{"Compare":{"op":"eq","left":{"Const":{"Int":1}},"right":{"Const":{"Int":1}}}},"actions":[{"Status":{"InitializeStatus":{}}}]},
                {"from":"owned","to":"owned","on_frame":false,"guard":{"Compare":{"op":"eq","left":{"Const":{"Int":1}},"right":{"Const":{"Int":1}}}},"actions":[{"Status":{"PlayMotion":{"motion":"wait","frame":{"Start":{"Const":{"Float":0.0}}},"rate":{"Const":{"Float":1.0}}}}}]},
                {"from":"owned","to":"owned","on_frame":false,"guard":"CancelEnabled","actions":[{"Status":{"TryCommonTransition":{"kind":"GroundWait"}}}]},
                {"from":"owned","to":"owned","on_frame":false,"guard":{"TermEnabled":{"symbol":"FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_JUMP_SQUAT"}},"actions":[{"Add":{"var":"count","value":{"Const":{"Int":1}}}},{"Status":"EnableCancel"}]}
            ]
        })).unwrap();
        assert!(program.validate().is_ok(), "{:?}", program.validate());
        install_compiled(vec![("main", program.clone())]);
        let status = 0x1f8;
        let mut snapshot = NativeSnapshot::default();
        snapshot.phase = Some(PhaseInput {
            fighter: "cloud".into(),
            status,
            status_frame: 1,
            motion_frame: 0.0,
            motion: 1,
            situation: None,
        });
        snapshot.cancel_enabled = Some(true);
        snapshot.term_enabled.insert(
            "FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_JUMP_SQUAT".into(),
            true,
        );
        runtime::status_inputs(106, "cloud", status, Phase::Pre)
            .unwrap()
            .unwrap();
        for phase in [Phase::Pre, Phase::MainEnter] {
            runtime::status_phase(
                106,
                "cloud",
                status,
                phase,
                &snapshot,
                |_| Ok(()),
                |_| Ok(false),
            )
            .unwrap();
        }
        let delivered = std::cell::RefCell::new(Vec::new());
        let result = runtime::status_phase(
            106,
            "cloud",
            status,
            Phase::MainLoop,
            &snapshot,
            |_| Ok(()),
            |action| {
                delivered.borrow_mut().push(action.clone());
                Ok(matches!(
                    action,
                    model::Action::Status(Command::TryCommonTransition { .. })
                ) && handled)
            },
        )
        .unwrap();
        assert_eq!(result, Some(handled));
        assert_eq!(delivered.borrow().len(), if handled { 1 } else { 2 });
        assert_eq!(
            runtime::read_controller_value(106, "cloud", "main", "count"),
            Some(model::Value::Int(i32::from(!handled)))
        );
        assert!(matches!(
            &delivered.borrow()[0],
            model::Action::Status(Command::TryCommonTransition {
                kind: CommonTransition::GroundWait
            })
        ));
        // A changed body cannot replace the program pinned by this entry.
        let mut edited = program.clone();
        edited.transitions[3].actions[0] = model::Action::Add {
            var: "count".into(),
            value: model::Expr::Const(model::Value::Int(100)),
        };
        let update = controller_bundle(1, vec![("main", edited)]);
        assert!(runtime::handle(&update).accepted);
        snapshot.phase.as_mut().unwrap().status_frame = 2;
        runtime::status_phase(
            106,
            "cloud",
            status,
            Phase::MainLoop,
            &snapshot,
            |_| Ok(()),
            |action| {
                Ok(matches!(
                    action,
                    model::Action::Status(Command::TryCommonTransition { .. })
                ) && handled)
            },
        )
        .unwrap();
        assert_eq!(
            runtime::read_controller_value(106, "cloud", "main", "count"),
            Some(model::Value::Int(if handled { 0 } else { 2 }))
        );
        // Body replacement is staged until Exit, including a disabled owner.
        let request = slight::gimmicks::controller_protocol::ControllerBundleRequest {
            version: slight::gimmicks::controller_protocol::REQUEST_VERSION,
            revision: 2,
            disabled_fighters: vec!["cloud".into()],
            charts: vec![],
        };
        let raw = slight::gimmicks::controller_protocol::encode_request(&request).unwrap();
        assert!(runtime::handle(&raw).accepted);
        assert!(runtime::owns_status(106, status));
        runtime::status_phase(
            106,
            "cloud",
            status,
            Phase::Exit,
            &snapshot,
            |_| Ok(()),
            |_| Ok(false),
        )
        .unwrap();
        assert!(!runtime::owns_status(106, status));
        program.custom_statuses.insert("other".into(), 1);
        let mut changed = request;
        changed.revision = 3;
        changed.disabled_fighters.clear();
        changed.charts = vec![slight::gimmicks::controller_protocol::ControllerProgram {
            chart_id: "main".into(),
            program,
        }];
        let raw = slight::gimmicks::controller_protocol::encode_request(&changed).unwrap();
        assert!(!runtime::handle(&raw).accepted);
    }
}

#[test]
fn receiver_deflection_dispatch_carries_power_for_live_and_compiled_roots() {
    use slight::gimmicks::{
        native_inputs::{DamageDirection, DamageInput},
        phase_commands::Command,
        phase_control,
    };
    let _serial = TEST_LOCK.lock().unwrap();
    for compiled in [false, true] {
        for (hook, direction) in [
            (model::HookName::OnReflect, DamageDirection::Reflect),
            (model::HookName::OnAbsorb, DamageDirection::Absorb),
        ] {
            reset();
            cloud(120, 0x12000, 0);
            let mut authored = ordinary_action_program(
                0,
                vec![model::Action::AddDamage {
                    amount: model::Expr::PendingDamage,
                }],
            );
            authored.transitions.truncate(1);
            let speed = NativeInput::Fighter {
                value: FighterValue::SpeedY,
                fighter: "cloud".into(),
            };
            authored.native_inputs = vec![speed.clone()];
            authored.transitions[0].guard = model::Predicate::Compare {
                op: model::CmpOp::Lt,
                left: model::Expr::NativeFloat(speed),
                right: model::Expr::Const(model::Value::Float(1.0)),
            };
            slight::gimmicks::native_values::set_test_sample(Some(0.0));
            authored.bindings = vec![model::HookBinding {
                hook,
                article: None,
                transitions: vec![0],
            }];
            if compiled {
                install_compiled(vec![("main", authored)]);
            } else {
                install_controller(1, vec![("main", authored)]);
            }
            let input = DamageInput::new(direction, 6.0)
                .unwrap()
                .with_projectile(true);
            runtime::dispatch_incoming(120, input, || false);
            assert!(phase_control::take_commands().is_empty());
            for _ in 0..2 {
                runtime::dispatch_incoming(120, input, || true);
                assert_eq!(
                    phase_control::take_commands(),
                    vec![Command::AddDamage(6.0)]
                );
            }
            slight::gimmicks::native_values::recycle_actor_after_sample();
            runtime::dispatch_incoming(120, input, || true);
            assert!(phase_control::take_commands().is_empty());
        }
    }
}

#[test]
fn form_switch_carries_once_and_rebirth_does_not_switch() {
    use slight::gimmicks::{phase_commands::Command, phase_control};
    let _serial = TEST_LOCK.lock().unwrap();
    for compiled in [true, false] {
        reset();
        fighter(201, 0x20100, 0, "eflame");
        fighter(202, 0x20200, 0, "elight");
        let mut outgoing = ordinary_action_program(7, vec![]);
        outgoing.fighter = "eflame".into();
        outgoing.id = "eflame-form".into();
        outgoing.vars[0].keep_across_form_switch = true;
        let mut incoming = ordinary_action_program(
            0,
            vec![model::Action::Add {
                var: "count".into(),
                value: model::Expr::Const(model::Value::Int(100)),
            }],
        );
        incoming.fighter = "elight".into();
        incoming.id = "elight-form".into();
        incoming.vars.push(model::VarDef {
            name: "switches".into(),
            kind: model::VarKind::Int,
            initial: model::Value::Int(0),
            keep_across_form_switch: false,
        });
        incoming.transitions[0].actions.push(model::Action::Add {
            var: "switches".into(),
            value: model::Expr::Const(model::Value::Int(1)),
        });
        incoming.vars[0].keep_across_form_switch = true;
        incoming.transitions[0]
            .actions
            .push(model::Action::AddDamage {
                amount: model::Expr::Const(model::Value::Float(10.0)),
            });
        incoming.bindings[0].hook = model::HookName::OnSwitch;
        if compiled {
            install_compiled(vec![("main", outgoing), ("main", incoming)]);
        } else {
            install_controller(1, vec![("main", outgoing), ("main", incoming)]);
        }
        let set_status = |boid, status| {
            let mut r = slight::agents::lookup(boid).unwrap();
            r.status_kind = status;
            slight::agents::set(r);
        };
        let tick = |boid| {
            runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, boid);
            runtime::observe_form(boid);
        };
        tick(202);
        assert_eq!(
            runtime::read_controller_value(202, "elight", "main", "count"),
            Some(model::Value::Int(0))
        );
        set_status(201, *smash::lib::lua_const::FIGHTER_STATUS_KIND_SPECIAL_LW);
        tick(201);
        // A global tick can see the still-inactive counterpart in WAIT. It must
        // not consume the handoff or send native actions before its own Main.
        runtime::dispatch(slight::gimmicks::dispatcher::GimmickHook::ChargeTick, 202);
        assert_eq!(
            runtime::read_controller_value(202, "elight", "main", "switches"),
            Some(model::Value::Int(0))
        );
        assert!(phase_control::take_commands().is_empty());
        // Main also visits the inactive counterpart in WAIT before activation.
        tick(202);
        assert_eq!(
            runtime::read_controller_value(202, "elight", "main", "switches"),
            Some(model::Value::Int(0))
        );
        assert!(phase_control::take_commands().is_empty());
        runtime::clear_boid(201); // Native teardown must preserve the entry handoff.
        set_status(
            202,
            *smash::lib::lua_const::FIGHTER_ELIGHT_STATUS_KIND_SPECIAL_LW_OUT,
        );
        tick(202);
        assert_eq!(
            runtime::read_controller_value(202, "elight", "main", "count"),
            Some(model::Value::Int(107))
        );
        assert_eq!(
            phase_control::take_commands(),
            vec![Command::AddDamage(10.0)]
        );
        tick(201);
        tick(202);
        assert_eq!(
            runtime::read_controller_value(202, "elight", "main", "count"),
            Some(model::Value::Int(107))
        );
        assert_eq!(
            runtime::read_controller_value(202, "elight", "main", "switches"),
            Some(model::Value::Int(1))
        );
        set_status(201, 0);
        tick(201);
        set_status(201, *smash::lib::lua_const::FIGHTER_STATUS_KIND_SPECIAL_LW);
        tick(201);
        runtime::clear_stock(201);
        set_status(202, *smash::lib::lua_const::FIGHTER_STATUS_KIND_REBIRTH);
        tick(202);
        runtime::clear_boid(201); // Native teardown must preserve the entry handoff.
        set_status(202, 0); // The native OUT status may finish before the object is sampled.
        tick(202);
        assert_eq!(
            runtime::read_controller_value(202, "elight", "main", "count"),
            Some(model::Value::Int(107))
        );
        assert_eq!(
            runtime::read_controller_value(202, "elight", "main", "switches"),
            Some(model::Value::Int(1))
        );
    }
}
