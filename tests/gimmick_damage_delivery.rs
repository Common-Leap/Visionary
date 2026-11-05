//! Exact production delivery module with scalar fixtures and a host mutex.
#![allow(dead_code)]

extern crate self as parking_lot;

pub struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
    pub fn new(value: T) -> Self {
        Self(std::sync::Mutex::new(value))
    }
    pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.0.lock().unwrap()
    }
}

#[path = "../plugins/slight_replica/src/slight/gimmicks/contact_snapshot_model.rs"]
mod contact_snapshot_model;
#[path = "../plugins/slight_replica/src/slight/gimmicks/damage_capture_model.rs"]
mod damage_capture_model;
#[path = "../plugins/slight_replica/src/slight/gimmicks/damage_commit_model.rs"]
mod damage_commit_model;
#[path = "../plugins/slight_replica/src/slight/gimmicks/damage_delivery_model.rs"]
mod damage_delivery_model;
#[path = "../plugins/slight_replica/src/slight/gimmicks/damage_dispatch.rs"]
mod damage_dispatch;
#[path = "../plugins/slight_replica/src/slight/gimmicks/damage_feed_model.rs"]
mod damage_feed_model;

use contact_snapshot_model::{Identity, Observation};
use damage_delivery_model::{Cursor, Direction, Event, Rejection};
use damage_dispatch::{deliver, register, Outcome};
use damage_feed_model::{Actor, Candidate, Stamp};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static EPOCH: AtomicU64 = AtomicU64::new(0);
static CURRENT: AtomicBool = AtomicBool::new(true);
static SEEN: std::sync::Mutex<Vec<Event>> = std::sync::Mutex::new(Vec::new());
static NESTED: std::sync::Mutex<Option<Outcome>> = std::sync::Mutex::new(None);

fn fixture() -> (Candidate, Stamp) {
    damage_dispatch::clear();
    assert!(!damage_dispatch::has_consumers());
    CURRENT.store(true, Ordering::SeqCst);
    SEEN.lock().unwrap().clear();
    let source = Actor {
        identity: Identity {
            object_id: 0,
            accessor: 0x1000,
            category: 0,
            kind: 62,
            entry: 0,
            founder: None,
        },
        stock_generation: 2,
    };
    let target = Actor {
        identity: Identity {
            object_id: 65536,
            accessor: 0x2000,
            category: 0,
            kind: 19,
            entry: 1,
            founder: None,
        },
        stock_generation: 3,
    };
    let sample = damage_commit_model::Sample {
        epoch: 0,
        frame: 20,
        index: 0,
        context_id: 0,
        caller_return_offset: Some(0x3febd8),
        before_bits: 999f32.to_bits(),
        pending_bits: 3f32.to_bits(),
        after_bits: 999f32.to_bits(),
        cap_bits: Some(999f32.to_bits()),
        suppressed: false,
        objects: [
            None,
            Some(Observation {
                identity: target.identity,
                percent_bits: Some(999f32.to_bits()),
            }),
            None,
        ],
    };
    let stamp = Stamp {
        epoch: EPOCH.fetch_add(1, Ordering::SeqCst) + 1,
        frame: 20,
        revision: 5,
    };
    (
        Candidate {
            shield: false,
            weapon: None,
            stamp,
            contact_index: 0,
            update_index: 2,
            source,
            target,
            source_status: Some(44),
            target_status: Some(486),
            before_bits: sample.before_bits,
            after_bits: sample.after_bits,
            cap_bits: sample.cap_bits.unwrap(),
            amount: damage_commit_model::confirmed_pending(&sample),
        },
        Stamp { frame: 21, ..stamp },
    )
}

// Handler lifecycle tests use a different victim for each independent delivery;
// exact callback-index replay is consumed by the production cursor.
fn next_victim(row: Candidate, contact_index: usize) -> Candidate {
    let mut target = row.target;
    target.identity.object_id += 65536;
    target.identity.entry += 1;
    Candidate {
        target,
        contact_index,
        ..row
    }
}

fn record(event: Event) {
    SEEN.lock().unwrap().push(event);
}
fn invalidate(event: Event) {
    record(event);
    CURRENT.store(false, Ordering::SeqCst);
}
fn is_current() -> bool {
    CURRENT.load(Ordering::SeqCst)
}

#[test]
fn each_root_retains_its_receiver_capture_status_and_missing_is_not_idle() {
    let _serial = TEST_LOCK.lock().unwrap();
    let (row, delivery) = fixture();
    register(Direction::Deal, record);
    register(Direction::Take, record);
    assert_eq!(
        deliver(row, delivery, || true),
        Outcome::Delivered { handlers: 2 }
    );
    let events = SEEN.lock().unwrap().clone();
    assert_eq!(events[0].receiver_status_at_contact(), Some(44));
    assert_eq!(events[1].receiver_status_at_contact(), Some(486));
    let next = Candidate {
        contact_index: 1,
        source_status: Some(0),
        target_status: None,
        ..next_victim(row, 1)
    };
    assert_eq!(
        deliver(next, delivery, || true),
        Outcome::Delivered { handlers: 2 }
    );
    let seen = SEEN.lock().unwrap();
    assert_eq!(seen[2].receiver_status_at_contact(), Some(0));
    assert_eq!(seen[3].receiver_status_at_contact(), None);
    assert_eq!(events[1].receiver_status_at_contact(), Some(486));
    drop(seen);
    let malformed = Candidate {
        contact_index: 2,
        target_status: Some(-1),
        ..next
    };
    assert_eq!(
        deliver(malformed, delivery, || true),
        Outcome::Rejected(Rejection::Invalid)
    );
}

#[test]
fn registration_changes_in_a_callback_affect_the_next_contact_without_deadlocking() {
    let _serial = TEST_LOCK.lock().unwrap();
    let (row, delivery) = fixture();
    static OLD: AtomicU64 = AtomicU64::new(0);
    static NEW_CALLS: AtomicU64 = AtomicU64::new(0);
    fn new_take(event: Event) {
        NEW_CALLS.fetch_add(1, Ordering::SeqCst);
        record(event);
    }
    fn change(event: Event) {
        damage_dispatch::unregister(OLD.load(Ordering::SeqCst));
        register(Direction::Take, new_take);
        record(event);
    }
    NEW_CALLS.store(0, Ordering::SeqCst);
    OLD.store(register(Direction::Take, record), Ordering::SeqCst);
    register(Direction::Deal, change);
    assert_eq!(
        deliver(row, delivery, || true),
        Outcome::Delivered { handlers: 2 }
    );
    assert_eq!(NEW_CALLS.load(Ordering::SeqCst), 0);
    assert_eq!(damage_dispatch::handler_count(Direction::Take), 1);
    assert_eq!(
        deliver(next_victim(row, 1), delivery, || true),
        Outcome::Delivered { handlers: 2 }
    );
    assert_eq!(NEW_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(SEEN.lock().unwrap().len(), 4);
}

#[test]
fn typed_capped_contact_delivers_deal_then_take_once_and_retains_both_stocks() {
    let _serial = TEST_LOCK.lock().unwrap();
    let (row, delivery) = fixture();
    let id = register(Direction::Deal, record);
    assert!(damage_dispatch::has_consumers());
    assert_eq!(id, register(Direction::Deal, record));
    register(Direction::Take, record);
    assert_eq!(damage_dispatch::handler_count(Direction::Deal), 1);
    assert_eq!(
        deliver(row, delivery, is_current),
        Outcome::Delivered { handlers: 2 }
    );
    let seen = SEEN.lock().unwrap();
    assert_eq!(seen.len(), 2);
    for (event, direction, receiver) in [
        (seen[0], Direction::Deal, row.source),
        (seen[1], Direction::Take, row.target),
    ] {
        assert_eq!(event.direction(), direction);
        assert_eq!(event.receiver(), receiver);
        assert_eq!(event.source(), row.source);
        assert_eq!(event.target(), row.target);
        assert_eq!(event.amount().unwrap().value(), 3.0);
        assert_eq!(event.stamp(), row.stamp);
        assert_eq!(event.delivery_stamp(), delivery);
        assert_eq!(event.contact_index(), 0);
        assert_eq!(event.update_index(), 2);
    }
    drop(seen);
    assert_eq!(
        deliver(row, delivery, is_current),
        Outcome::Rejected(Rejection::Consumed)
    );
    // A handler clear is not permission to replay an already consumed contact.
    damage_dispatch::clear();
    assert!(!damage_dispatch::has_consumers());
    register(Direction::Take, record);
    assert_eq!(
        deliver(row, delivery, is_current),
        Outcome::Rejected(Rejection::Consumed)
    );
    assert!(damage_dispatch::unregister(register(
        Direction::Deal,
        record
    )));
    assert!(!damage_dispatch::unregister(id));
    assert!(damage_dispatch::has_consumers());
    damage_dispatch::clear();
    let only = register(Direction::Deal, record);
    assert_eq!(only, register(Direction::Deal, record));
    assert!(damage_dispatch::unregister(only));
    assert!(!damage_dispatch::has_consumers());
    assert!(!damage_dispatch::unregister(only));
    assert!(!damage_dispatch::has_consumers());
}

#[test]
fn actor_change_before_or_between_handlers_aborts_without_retrying_a_partial_contact() {
    let _serial = TEST_LOCK.lock().unwrap();
    let (row, delivery) = fixture();
    register(Direction::Deal, invalidate);
    register(Direction::Deal, record);
    register(Direction::Take, record);
    CURRENT.store(false, Ordering::SeqCst);
    assert_eq!(
        deliver(row, delivery, is_current),
        Outcome::Invalidated { handlers: 0 }
    );
    CURRENT.store(true, Ordering::SeqCst);
    assert_eq!(
        deliver(row, delivery, is_current),
        Outcome::Rejected(Rejection::Consumed)
    );
    let row = next_victim(row, 1);
    assert_eq!(
        deliver(row, delivery, is_current),
        Outcome::Invalidated { handlers: 1 }
    );
    assert_eq!(SEEN.lock().unwrap().len(), 1);
    CURRENT.store(true, Ordering::SeqCst);
    assert_eq!(
        deliver(row, delivery, is_current),
        Outcome::Rejected(Rejection::Consumed)
    );
}

fn reenter(event: Event) {
    record(event);
    let row = Candidate {
        shield: event.contact().shield,
        weapon: None,
        stamp: event.stamp(),
        contact_index: event.contact_index() + 1,
        update_index: event.update_index(),
        source: event.source(),
        target: event.target(),
        source_status: event.contact().source_status,
        target_status: event.contact().target_status,
        amount: event.amount(),
        before_bits: 0,
        after_bits: 0,
        cap_bits: 0,
    };
    *NESTED.lock().unwrap() = Some(deliver(
        row,
        Stamp {
            frame: row.stamp.frame + 1,
            ..row.stamp
        },
        || true,
    ));
}

#[test]
fn nested_contact_is_rejected_without_consuming_the_next_ordinary_contact() {
    let _serial = TEST_LOCK.lock().unwrap();
    let (row, delivery) = fixture();
    let id = register(Direction::Deal, reenter);
    register(Direction::Take, record);
    assert_eq!(
        deliver(row, delivery, || true),
        Outcome::Delivered { handlers: 2 }
    );
    assert_eq!(
        *NESTED.lock().unwrap(),
        Some(Outcome::Rejected(Rejection::Reentrant))
    );
    damage_dispatch::unregister(id);
    assert_eq!(
        deliver(next_victim(row, 1), delivery, || true),
        Outcome::Delivered { handlers: 1 }
    );
}

fn clear_and_reenter(event: Event) {
    damage_dispatch::clear();
    reenter(event);
}

#[test]
fn clearing_inside_a_callback_invalidates_the_batch_and_keeps_the_nested_fence() {
    let _serial = TEST_LOCK.lock().unwrap();
    let (row, delivery) = fixture();
    register(Direction::Deal, clear_and_reenter);
    register(Direction::Take, record);
    assert_eq!(
        deliver(row, delivery, || true),
        Outcome::Invalidated { handlers: 1 }
    );
    assert_eq!(
        *NESTED.lock().unwrap(),
        Some(Outcome::Rejected(Rejection::Reentrant))
    );
    assert_eq!(SEEN.lock().unwrap().len(), 1);
    register(Direction::Take, record);
    assert_eq!(
        deliver(row, delivery, || true),
        Outcome::Rejected(Rejection::Consumed)
    );
    assert_eq!(
        deliver(next_victim(row, 1), delivery, || true),
        Outcome::Delivered { handlers: 1 }
    );
}

#[test]
fn final_handler_changes_and_panics_cannot_replay_or_leave_the_dispatcher_locked() {
    let _serial = TEST_LOCK.lock().unwrap();
    let (row, delivery) = fixture();
    let id = register(Direction::Take, invalidate);
    assert_eq!(
        deliver(row, delivery, is_current),
        Outcome::Invalidated { handlers: 1 }
    );
    damage_dispatch::unregister(id);
    fn panic_handler(_: Event) {
        panic!("consumer failure");
    }
    let id = register(Direction::Take, panic_handler);
    let row = next_victim(row, 1);
    assert!(std::panic::catch_unwind(|| deliver(row, delivery, || true)).is_err());
    damage_dispatch::unregister(id);
    assert_eq!(
        deliver(row, delivery, || true),
        Outcome::Rejected(Rejection::Consumed)
    );
    assert_eq!(
        deliver(next_victim(row, 2), delivery, || true),
        Outcome::Delivered { handlers: 0 }
    );
}

#[test]
fn cursor_rejects_old_epochs_order_lifetime_late_frames_and_malformed_actors() {
    let _serial = TEST_LOCK.lock().unwrap();
    let (row, delivery) = fixture();
    for stamp in [
        Stamp {
            epoch: delivery.epoch + 1,
            ..delivery
        },
        Stamp {
            revision: 6,
            ..delivery
        },
    ] {
        assert_eq!(Cursor::new().begin(row, stamp), Err(Rejection::Lifetime));
    }
    for frame in [20, 22, u64::MAX] {
        assert_eq!(
            Cursor::new().begin(row, Stamp { frame, ..delivery }),
            Err(Rejection::Late)
        );
    }
    for mut bad in [row; 6].into_iter().enumerate().map(|(i, mut bad)| {
        match i {
            0 => bad.source.identity.accessor = 0,
            1 => bad.target.stock_generation = u32::MAX,
            2 => bad.target.identity.category = 1,
            3 => bad.target = bad.source,
            4 => bad.target.identity.entry = bad.source.identity.entry,
            _ => bad.contact_index = damage_feed_model::CAPACITY,
        }
        bad
    }) {
        assert_eq!(Cursor::new().begin(bad, delivery), Err(Rejection::Invalid));
        bad.update_index = damage_feed_model::CAPACITY;
        assert_eq!(Cursor::new().begin(bad, delivery), Err(Rejection::Invalid));
    }
    let mut cursor = Cursor::new();
    cursor.begin(next_victim(row, 2), delivery).unwrap();
    assert_eq!(cursor.begin(row, delivery), Err(Rejection::Reentrant));
    cursor.finish();
    assert_eq!(cursor.begin(row, delivery), Err(Rejection::Consumed));
    let next = Candidate {
        stamp: Stamp {
            frame: 21,
            ..row.stamp
        },
        ..row
    };
    cursor
        .begin(
            next,
            Stamp {
                frame: 22,
                ..delivery
            },
        )
        .unwrap();
    cursor.finish();
    assert_eq!(cursor.begin(row, delivery), Err(Rejection::Consumed));
    let old_epoch = Candidate {
        stamp: Stamp {
            epoch: row.stamp.epoch - 1,
            frame: 100,
            ..row.stamp
        },
        ..row
    };
    assert_eq!(
        cursor.begin(
            old_epoch,
            Stamp {
                frame: 101,
                ..old_epoch.stamp
            }
        ),
        Err(Rejection::Consumed)
    );
}

#[test]
fn shield_dispatch_preserves_each_contact_and_cancels_defender_after_attacker_invalidation() {
    let _serial = TEST_LOCK.lock().unwrap();
    let (mut row, delivery) = fixture();
    row.shield = true;
    row.amount = damage_commit_model::NativeDamageAmount::blocked_power(4.5f32.to_bits());
    register(Direction::AttackShielded, record);
    register(Direction::ShieldHit, record);
    for index in 0..3 {
        row.contact_index = index;
        row.update_index = index;
        assert_eq!(
            deliver(row, delivery, is_current),
            Outcome::Delivered { handlers: 2 }
        );
    }
    let seen = SEEN.lock().unwrap().clone();
    assert_eq!(seen.len(), 6);
    for pair in seen.chunks_exact(2) {
        assert_eq!(pair[0].direction(), Direction::AttackShielded);
        assert_eq!(pair[1].direction(), Direction::ShieldHit);
        assert_eq!(pair[0].receiver(), row.source);
        assert_eq!(pair[1].receiver(), row.target);
        assert_eq!(pair[1].amount().unwrap().value(), 4.5);
    }
    damage_dispatch::clear();
    register(Direction::AttackShielded, invalidate);
    register(Direction::ShieldHit, record);
    row.contact_index += 1;
    assert_eq!(
        deliver(row, delivery, is_current),
        Outcome::Invalidated { handlers: 1 }
    );
    assert_eq!(SEEN.lock().unwrap().len(), 7);
}

fn reenter_other_stream(event: Event) {
    let mut row = event.contact();
    row.shield = !row.shield;
    row.contact_index += 1;
    *NESTED.lock().unwrap() = Some(deliver(row, event.delivery_stamp(), || true));
}

#[test]
fn a_damage_handler_cannot_nest_shield_delivery() {
    let _serial = TEST_LOCK.lock().unwrap();
    let (row, delivery) = fixture();
    *NESTED.lock().unwrap() = None;
    register(Direction::Deal, reenter_other_stream);
    assert_eq!(
        deliver(row, delivery, || true),
        Outcome::Delivered { handlers: 1 }
    );
    assert_eq!(
        *NESTED.lock().unwrap(),
        Some(Outcome::Rejected(Rejection::Reentrant))
    );
}
