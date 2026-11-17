#[allow(dead_code)]
#[path = "../plugins/slight_replica/src/slight/gimmicks/contact_snapshot_model.rs"]
mod contact_snapshot_model;
#[allow(dead_code)]
#[path = "../plugins/slight_replica/src/slight/gimmicks/damage_capture_model.rs"]
mod damage_capture_model;
#[path = "../plugins/slight_replica/src/slight/gimmicks/damage_commit_model.rs"]
mod damage_commit_model;

use contact_snapshot_model::{Identity, Observation};
use damage_commit_model::{agreement, confirmed_pending, Agreement, CallKey, PendingStore, Sample};

fn sample(before: f32, pending: f32, after: f32) -> Sample {
    Sample {
        epoch: 0,
        frame: 20,
        index: 0,
        context_id: 65536,
        caller_return_offset: None,
        before_bits: before.to_bits(),
        pending_bits: pending.to_bits(),
        after_bits: after.to_bits(),
        cap_bits: None,
        suppressed: false,
        objects: [
            None,
            Some(Observation {
                identity: Identity {
                    object_id: 0,
                    accessor: 0x1000,
                    category: 0,
                    kind: 0,
                    entry: 0,
                    founder: None,
                },
                percent_bits: Some(after.to_bits()),
            }),
            None,
        ],
    }
}

#[test]
fn confirmed_pending_preserves_damage_units_at_the_native_cap_and_rounding_boundary() {
    let mut row = sample(999.0, 3.0, 999.0);
    row.caller_return_offset = Some(0x3febd8);
    row.cap_bits = Some(999.0f32.to_bits());
    assert_eq!(agreement(&row), Agreement::Clamped);
    assert_eq!(Agreement::Clamped.name(), "clamped");
    assert_eq!(confirmed_pending(&row).unwrap().value(), 3.0);
    assert_eq!(confirmed_pending(&row).unwrap().bits(), 3.0f32.to_bits());
    row.before_bits = 998.0f32.to_bits();
    assert_eq!(agreement(&row), Agreement::Clamped);
    assert_eq!(confirmed_pending(&row).unwrap().value(), 3.0);
    row = sample(999.0, 0.000001, 999.0);
    row.caller_return_offset = Some(0x3febd8);
    row.cap_bits = Some(1000.0f32.to_bits());
    assert_eq!(agreement(&row), Agreement::Rounded);
    assert_eq!(Agreement::Rounded.name(), "rounded");
    assert_eq!(confirmed_pending(&row).unwrap().value(), 0.000001);
    for cap in [f32::NAN, f32::INFINITY, -1.0, 998.0] {
        row.cap_bits = Some(cap.to_bits());
        assert_eq!(agreement(&row), Agreement::Mismatch);
        assert_eq!(confirmed_pending(&row), None);
    }
}

#[test]
fn pending_amount_requires_measured_bound_and_reviewed_caller_and_excludes_adjustments() {
    let mut row = sample(0.0, 3.0, 3.0);
    assert_eq!(confirmed_pending(&row), None);
    row.cap_bits = Some(999.0f32.to_bits());
    for caller in [None, Some(0x3fede8), Some(0x404008)] {
        row.caller_return_offset = caller;
        assert_eq!(confirmed_pending(&row), None);
    }
    row.caller_return_offset = Some(0x3febd8);
    assert_eq!(confirmed_pending(&row).unwrap().value(), 3.0);
    row = Sample {
        caller_return_offset: Some(0x3febd8),
        cap_bits: Some(999.0f32.to_bits()),
        ..sample(0.0, 10_000_000.0, 999.0)
    };
    assert_eq!(agreement(&row), Agreement::Clamped);
    assert_eq!(confirmed_pending(&row), None);
}

#[test]
fn native_percent_add_requires_exact_arithmetic_and_independent_getter_agreement() {
    let row = sample(6.0, 2.64, 6.0 + 2.64);
    assert_eq!(agreement(&row), Agreement::Added);
    assert_eq!(Agreement::Added.name(), "added");
    let mut changed = row;
    changed.objects[1].as_mut().unwrap().percent_bits = Some(8.6f32.to_bits());
    assert_eq!(agreement(&changed), Agreement::Mismatch);
    changed = row;
    changed.after_bits += 1;
    changed.objects[1].as_mut().unwrap().percent_bits = Some(changed.after_bits);
    assert_eq!(agreement(&changed), Agreement::Mismatch);
    changed.objects[1] = None;
    assert_eq!(agreement(&changed), Agreement::Unknown);
    changed = row;
    changed.objects[1].as_mut().unwrap().identity.accessor = 0;
    assert_eq!(agreement(&changed), Agreement::Mismatch);
}

#[test]
fn zero_suppressed_healing_nonfinite_and_rounded_away_values_are_not_positive_adds() {
    assert_eq!(agreement(&sample(6.0, 0.0, 6.0)), Agreement::Unchanged);
    let mut suppressed = sample(6.0, 2.64, 6.0);
    suppressed.suppressed = true;
    assert_eq!(agreement(&suppressed), Agreement::Suppressed);
    suppressed.after_bits = 8.64f32.to_bits();
    suppressed.objects[1].as_mut().unwrap().percent_bits = Some(suppressed.after_bits);
    assert_eq!(agreement(&suppressed), Agreement::Mismatch);
    for row in [
        sample(6.0, -1.0, 5.0),
        sample(6.0, f32::NAN, 6.0),
        sample(6.0, f32::INFINITY, 6.0),
        sample(-1.0, 2.0, 1.0),
        sample(1_000_000.0, 1.0, 1_000_001.0),
        sample(6.0, f32::MIN_POSITIVE, 6.0),
    ] {
        assert_eq!(agreement(&row), Agreement::Mismatch);
    }
}

#[test]
fn native_call_pairs_require_the_same_stack_module_data_epoch_and_frame() {
    let mut calls = PendingStore::new();
    let row = sample(0.0, 6.0, 6.0);
    let key = CallKey {
        epoch: 0,
        frame: 20,
        stack: 0x8000,
        module: 0x2000,
        data: 0x3000,
    };
    assert!(calls.begin(key, row));
    for changed in [
        CallKey {
            stack: 0x8010,
            ..key
        },
        CallKey {
            module: 0x2010,
            ..key
        },
        CallKey {
            data: 0x3010,
            ..key
        },
        CallKey { epoch: 1, ..key },
        CallKey { frame: 21, ..key },
    ] {
        assert_eq!(calls.end(changed), None);
    }
    assert_eq!(calls.end(key), Some(row));
    assert_eq!(calls.end(key), None);
    assert!(!calls.begin(CallKey { stack: 0, ..key }, row));
    assert!(!calls.begin(
        CallKey {
            stack: 0x8008,
            ..key
        },
        row
    ));
    assert!(!calls.begin(key, Sample { epoch: 1, ..row }));
    assert!(calls.begin(key, row));
    calls.expire(0, 21);
    assert_eq!(calls.end(key), None);
    assert!(calls.begin(key, row));
    calls.expire(1, 20);
    assert_eq!(calls.end(key), None);
}

#[test]
fn native_call_pair_capacity_and_nested_order_never_borrow_another_reservation() {
    let mut calls = PendingStore::new();
    let row = sample(0.0, 6.0, 6.0);
    let key = CallKey {
        epoch: 0,
        frame: 20,
        stack: 0x8000,
        module: 0x2000,
        data: 0x3000,
    };
    for index in 0..16 {
        assert!(calls.begin(
            CallKey {
                stack: key.stack + u64::from(index) * 16,
                ..key
            },
            Sample { index, ..row }
        ));
    }
    assert!(!calls.begin(
        CallKey {
            stack: 0x9000,
            ..key
        },
        row
    ));
    assert!(calls.begin(key, Sample { index: 100, ..row }));
    for index in (1..16).rev() {
        assert_eq!(
            calls
                .end(CallKey {
                    stack: key.stack + u64::from(index) * 16,
                    ..key
                })
                .unwrap()
                .index,
            index
        );
    }
    assert_eq!(calls.end(key).unwrap().index, 100);
}
