#[allow(dead_code)]
#[path = "../plugins/slight_replica/src/slight/gimmicks/contact_snapshot_model.rs"]
mod contact_snapshot_model;
#[allow(dead_code)]
#[path = "../plugins/slight_replica/src/slight/gimmicks/damage_capture_model.rs"]
mod damage_capture_model;
#[allow(dead_code)]
#[path = "../plugins/slight_replica/src/slight/gimmicks/damage_commit_model.rs"]
mod damage_commit_model;
#[path = "../plugins/slight_replica/src/slight/gimmicks/damage_feed_model.rs"]
mod damage_feed_model;

use contact_snapshot_model::{Identity, Observation};
use damage_feed_model::*;

const STAMP: Stamp = Stamp {
    epoch: 1,
    frame: 20,
    revision: 5,
};
const DELIVERY: Stamp = Stamp { frame: 21, ..STAMP };

fn pair(before: f32, amount: f32) -> (Contact, Update) {
    let source = Actor {
        identity: Identity {
            object_id: 0,
            accessor: 0x1000,
            category: 0,
            kind: 0,
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
            kind: 13,
            entry: 1,
            founder: None,
        },
        stock_generation: 3,
    };
    let actors = [Some(source), Some(target)];
    let objects = [
        Some(Observation {
            identity: source.identity,
            percent_bits: Some(0),
        }),
        Some(Observation {
            identity: target.identity,
            percent_bits: Some(before.to_bits()),
        }),
        None,
    ];
    let contact = Contact {
        raw: damage_capture_model::RawSample {
            epoch: 1,
            frame: 20,
            sample_index: 0,
            object_id_1: 0,
            object_id_2: 65536,
            amount_bits: amount.to_bits(),
            caller_return_offset: Some(0x6c2f10),
            ..Default::default()
        },
        objects,
        actors,
        statuses: [Some(44), Some(486)],
    };
    let mut after = objects;
    after[1].as_mut().unwrap().percent_bits = Some((before + amount).to_bits());
    let update = Update {
        sample: damage_commit_model::Sample {
            epoch: 1,
            frame: 20,
            index: 0,
            context_id: 0,
            caller_return_offset: Some(0x3febd8),
            before_bits: before.to_bits(),
            pending_bits: amount.to_bits(),
            after_bits: (before + amount).to_bits(),
            cap_bits: Some(999.0f32.to_bits()),
            suppressed: false,
            objects: after,
        },
        actors,
    };
    (contact, update)
}

fn queue() -> Queue {
    let mut q = Queue::new();
    q.begin(STAMP).unwrap();
    q
}

fn add(q: &mut Queue, contact: Contact, update: Update) {
    let a = q.reserve(STAMP, Stream::Contact).unwrap();
    let b = q.reserve(STAMP, Stream::Update).unwrap();
    q.contact(a, contact).unwrap();
    q.update(b, update).unwrap();
}

fn reason(q: &mut Queue) -> Exclusion {
    q.seal(DELIVERY).unwrap();
    match q.next(DELIVERY, |_| true).unwrap().unwrap() {
        Decision::Excluded { reason, .. } => {
            assert!(!reason.name().is_empty());
            reason
        }
        other => panic!("unexpected decision {other:?}"),
    }
}

#[test]
fn capture_statuses_stay_with_their_own_contact_and_do_not_change_actor_lifetime() {
    let mut q = queue();
    let (first, update) = pair(0.0, 2.4);
    add(&mut q, first, update);
    let (mut second, update) = pair(2.4, 3.0);
    second.statuses = [Some(0), Some(72)];
    add(&mut q, second, update);
    q.seal(DELIVERY).unwrap();
    for expected in [[Some(44), Some(486)], [Some(0), Some(72)]] {
        let Decision::Candidate(row) = q.next(DELIVERY, |_| true).unwrap().unwrap() else {
            panic!("status change must not become a stock change")
        };
        assert_eq!([row.source_status, row.target_status], expected);
    }
    assert_eq!(q.next(DELIVERY, |_| true), Ok(None));
    let mut q = queue();
    let (mut contact, update) = pair(0.0, 3.0);
    contact.statuses = [None, Some(-1)];
    add(&mut q, contact, update);
    q.seal(DELIVERY).unwrap();
    let Decision::Candidate(row) = q.next(DELIVERY, |_| true).unwrap().unwrap() else {
        panic!()
    };
    assert_eq!([row.source_status, row.target_status], [None, None]);
}

#[test]
fn complete_frame_delivers_each_separate_contact_once_without_aggregating() {
    let mut q = queue();
    let (c, u) = pair(0.0, 2.4);
    add(&mut q, c, u);
    let (c, u) = pair(2.4, 3.0);
    add(&mut q, c, u);
    assert_eq!(q.counts(), [2, 2]);
    assert_eq!(q.stamp(), Some(STAMP));
    assert_eq!(q.seal(STAMP), Err(Fault::NotClosed));
    assert_eq!(q.next(DELIVERY, |_| true), Err(Fault::NotClosed));
    q.seal(DELIVERY).unwrap();
    for (index, before, amount) in [(0, 0.0f32, 2.4f32), (1, 2.4, 3.0)] {
        let Decision::Candidate(candidate) = q.next(DELIVERY, |_| true).unwrap().unwrap() else {
            panic!()
        };
        assert_eq!(candidate.contact_index, index);
        assert_eq!(candidate.update_index, index);
        assert_eq!(candidate.before_bits, before.to_bits());
        assert_eq!(candidate.amount.unwrap().bits(), amount.to_bits());
    }
    assert_eq!(q.next(DELIVERY, |_| true), Ok(None));
}

#[test]
fn cap_contact_carries_pending_units_and_adjustment_caller_never_borrows_contact_credit() {
    let mut q = queue();
    let (c, mut u) = pair(999.0, 3.0);
    u.sample.after_bits = 999.0f32.to_bits();
    u.sample.objects[1].as_mut().unwrap().percent_bits = Some(u.sample.after_bits);
    add(&mut q, c, u);
    q.seal(DELIVERY).unwrap();
    let Decision::Candidate(row) = q.next(DELIVERY, |_| true).unwrap().unwrap() else {
        panic!()
    };
    assert_eq!(row.before_bits, row.after_bits);
    assert_eq!(row.amount.unwrap().value(), 3.0);
    assert_eq!(row.cap_bits, 999.0f32.to_bits());
    let mut q = queue();
    let (c, mut u) = pair(0.0, 3.0);
    u.sample.caller_return_offset = Some(0x3fede8);
    add(&mut q, c, u);
    assert_eq!(reason(&mut q), Exclusion::Unmatched);
}

#[test]
fn frame_capacity_is_reused_after_hundreds_of_contacts_without_a_match_budget() {
    let mut q = Queue::new();
    for frame in 20..532 {
        let stamp = Stamp { frame, ..STAMP };
        q.begin(stamp).unwrap();
        let (mut contact, mut update) = pair(0.0, 6.0);
        contact.raw.frame = frame;
        update.sample.frame = frame;
        let a = q.reserve(stamp, Stream::Contact).unwrap();
        let b = q.reserve(stamp, Stream::Update).unwrap();
        q.contact(a, contact).unwrap();
        q.update(b, update).unwrap();
        let delivery = Stamp {
            frame: frame + 1,
            ..stamp
        };
        q.seal(delivery).unwrap();
        assert!(matches!(
            q.next(delivery, |_| true),
            Ok(Some(Decision::Candidate(_)))
        ));
        assert_eq!(q.next(delivery, |_| true), Ok(None));
    }
}

#[test]
fn trailing_missing_capture_duplicate_completion_or_overflow_rejects_entire_frame() {
    for stream in [Stream::Contact, Stream::Update] {
        let mut q = queue();
        let (c, u) = pair(0.0, 6.0);
        add(&mut q, c, u);
        q.reserve(STAMP, stream).unwrap();
        assert_eq!(q.seal(DELIVERY), Err(Fault::Incomplete));
        assert_eq!(q.next(DELIVERY, |_| true), Err(Fault::Incomplete));
    }
    let mut q = queue();
    let (c, u) = pair(0.0, 6.0);
    let ticket = q.reserve(STAMP, Stream::Contact).unwrap();
    q.contact(ticket, c).unwrap();
    assert_eq!(q.contact(ticket, c), Err(Fault::Duplicate));
    assert_eq!(q.seal(DELIVERY), Err(Fault::Duplicate));
    let mut q = queue();
    for _ in 0..CAPACITY {
        add(&mut q, c, u);
    }
    assert_eq!(q.reserve(STAMP, Stream::Contact), Err(Fault::Capacity));
    assert_eq!(q.seal(DELIVERY), Err(Fault::Capacity));
    let mut q = queue();
    add(&mut q, c, u);
    q.lose();
    assert_eq!(q.seal(DELIVERY), Err(Fault::CaptureLost));
    assert_eq!(Fault::CaptureLost.name(), "capture_lost");
}

#[test]
fn stale_tickets_never_populate_a_reused_frame_and_bad_stream_poisons_current_frame() {
    let mut q = queue();
    let ticket = q.reserve(STAMP, Stream::Contact).unwrap();
    assert_eq!(q.begin(STAMP), Err(Fault::StaleTicket));
    let next = Stamp { frame: 21, ..STAMP };
    q.begin(next).unwrap();
    let current = q.reserve(next, Stream::Contact).unwrap();
    let (mut c, _) = pair(0.0, 6.0);
    assert_eq!(q.contact(ticket, c), Err(Fault::StaleTicket));
    c.raw.frame = 21;
    q.contact(current, c).unwrap();
    q.seal(Stamp { frame: 22, ..STAMP }).unwrap();
    let mut q = queue();
    let ticket = q.reserve(STAMP, Stream::Update).unwrap();
    assert_eq!(q.contact(ticket, c), Err(Fault::StaleTicket));
    assert_eq!(q.seal(DELIVERY), Err(Fault::CaptureLost));
}

#[test]
fn crossing_lifecycle_or_match_or_delaying_a_frame_cannot_deliver_or_be_retried() {
    for delivery in [
        Stamp {
            revision: 6,
            ..DELIVERY
        },
        Stamp {
            epoch: 2,
            ..DELIVERY
        },
        Stamp {
            frame: 22,
            ..DELIVERY
        },
    ] {
        let mut q = queue();
        let (c, u) = pair(0.0, 6.0);
        add(&mut q, c, u);
        assert!(q.seal(delivery).is_err());
        assert!(q.next(DELIVERY, |_| true).is_err());
    }
    let mut q = queue();
    let (c, u) = pair(0.0, 6.0);
    add(&mut q, c, u);
    q.seal(DELIVERY).unwrap();
    assert_eq!(
        q.next(
            Stamp {
                revision: 6,
                ..DELIVERY
            },
            |_| true
        ),
        Err(Fault::LifetimeChanged)
    );
    assert_eq!(q.next(DELIVERY, |_| true), Err(Fault::LifetimeChanged));
}

#[test]
fn stock_and_native_identity_must_match_at_both_captures_and_delivery() {
    for side in 0..2 {
        let mut q = queue();
        let (c, mut u) = pair(0.0, 6.0);
        u.actors[side].as_mut().unwrap().stock_generation += 1;
        add(&mut q, c, u);
        assert_eq!(reason(&mut q), Exclusion::Lifetime);
        let mut q = queue();
        let (mut c, u) = pair(0.0, 6.0);
        c.actors[side].as_mut().unwrap().identity.accessor += 8;
        add(&mut q, c, u);
        assert_eq!(reason(&mut q), Exclusion::Lifetime);
        let mut q = queue();
        let (c, u) = pair(0.0, 6.0);
        add(&mut q, c, u);
        q.seal(DELIVERY).unwrap();
        assert_eq!(
            q.next(DELIVERY, |actor| actor != c.actors[side].unwrap()),
            Ok(Some(Decision::Excluded {
                index: 0,
                reason: Exclusion::Lifetime
            }))
        );
    }
}

#[test]
fn duplicate_matches_and_reversed_contact_order_never_grant_credit() {
    let (c, u) = pair(0.0, 6.0);
    let mut q = queue();
    add(&mut q, c, u);
    let ticket = q.reserve(STAMP, Stream::Contact).unwrap();
    q.contact(ticket, c).unwrap();
    assert_eq!(reason(&mut q), Exclusion::Ambiguous);
    let mut q = queue();
    add(&mut q, c, u);
    let ticket = q.reserve(STAMP, Stream::Update).unwrap();
    q.update(ticket, u).unwrap();
    assert_eq!(reason(&mut q), Exclusion::Ambiguous);
    let mut q = queue();
    let (c2, u2) = pair(6.0, 3.0);
    add(&mut q, c, u2);
    add(&mut q, c2, u);
    assert_eq!(reason(&mut q), Exclusion::Order);
}

#[test]
fn raw_positive_values_and_owner_entry_fallbacks_cannot_admit_unsupported_contacts() {
    for (id, expected) in [
        (0x40000000, Exclusion::Item),
        (0x10000000, Exclusion::WeaponCredit),
        (0x20000000, Exclusion::Unsupported),
        (65536, Exclusion::SelfContact),
    ] {
        let mut q = queue();
        let (mut c, u) = pair(0.0, 6.0);
        c.raw.object_id_1 = id;
        add(&mut q, c, u);
        assert_eq!(reason(&mut q), expected);
    }
    let mut q = queue();
    let (mut c, u) = pair(0.0, 6.0);
    c.raw.caller_return_offset = None;
    add(&mut q, c, u);
    assert_eq!(reason(&mut q), Exclusion::Caller);
    let mut q = queue();
    let (c, mut u) = pair(0.0, 6.0);
    u.sample.after_bits += 1;
    add(&mut q, c, u);
    assert_eq!(reason(&mut q), Exclusion::Unmatched);
}
