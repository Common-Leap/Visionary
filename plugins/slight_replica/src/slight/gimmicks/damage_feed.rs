//! Demand-driven native contact feed for controller damage hooks.

use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::{damage_capture, damage_commit_model as commit, damage_feed_model as model};

static ENABLED: AtomicBool = AtomicBool::new(false);
static REVISION: AtomicU64 = AtomicU64::new(0);
static LOST_FRAME: AtomicU64 = AtomicU64::new(0);
static STATE: Mutex<State> = Mutex::new(State {
    queue: model::Queue::new(),
    pending: [None; 16],
});
type Pending = (commit::CallKey, model::Ticket, [Option<model::Actor>; 2]);
struct State {
    queue: model::Queue,
    pending: [Option<Pending>; 16],
}

pub(super) fn capture_is_complete(frame: u64) -> bool {
    LOST_FRAME.load(Ordering::Acquire) < frame
}

pub(super) fn revision() -> u64 {
    REVISION.load(Ordering::Acquire)
}

/// Called immediately before a validated controller configuration changes,
/// while its owner holds the configuration lock. Atomic only: a queued contact
/// must not be reinterpreted by a configuration installed after capture.
pub(super) fn configuration_changed() {
    REVISION.fetch_add(1, Ordering::AcqRel);
}

pub(super) fn is_enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

pub(super) fn disable() {
    ENABLED.store(false, Ordering::Release);
    REVISION.fetch_add(1, Ordering::AcqRel);
}

/// Framework lifecycle callbacks run outside the native diagnostic callbacks.
/// A change to any fighter conservatively invalidates every contact in flight.
pub(super) fn lifecycle() {
    REVISION.fetch_add(1, Ordering::AcqRel);
}

fn stamp(epoch: u64, frame: u64) -> model::Stamp {
    model::Stamp {
        epoch,
        frame,
        revision: revision(),
    }
}

pub(super) fn event_is_current(event: super::damage_dispatch::Event) -> bool {
    delivery_is_current(event.contact(), event.delivery_stamp())
}

pub(super) fn delivery_is_current(row: model::Candidate, delivery: model::Stamp) -> bool {
    let context_is_current = || {
        is_enabled()
            && damage_capture::capture_context()
                .is_some_and(|(epoch, frame, _)| epoch == delivery.epoch && frame == delivery.frame)
            && stamp(delivery.epoch, delivery.frame) == delivery
            && capture_is_complete(row.stamp.frame)
    };
    // Weapon identity/owner were paired at capture and commit. A projectile
    // may despawn on impact before this sealed-frame delivery; only the two
    // credited fighter stocks must still exist, and no weapon pointer is used.
    context_is_current()
        && [row.source, row.target].into_iter().all(|actor| {
            // Resolve by BOID first. The captured accessor is an expectation,
            // never a pointer dereferenced from the retained payload.
            unsafe { damage_capture::read_native_object(actor.identity.object_id) }.and_then(
                |object| {
                    damage_capture::published_actor(delivery.epoch, delivery.frame, object.identity)
                },
            ) == Some(actor)
        })
        && context_is_current()
}

pub(super) fn enable(epoch: u64, frame: u64) {
    if let Some(mut state) = STATE.try_lock() {
        if state.queue.begin(stamp(epoch, frame)).is_ok() {
            ENABLED.store(true, Ordering::Release);
        }
    }
}

pub(super) fn lose(frame: u64) {
    if ENABLED.load(Ordering::Acquire) {
        LOST_FRAME.fetch_max(frame, Ordering::AcqRel);
    }
}

pub(super) fn reserve_contact(epoch: u64, frame: u64) -> Option<model::Ticket> {
    if !ENABLED.load(Ordering::Acquire) {
        return None;
    }
    let ticket = STATE.try_lock().and_then(|mut state| {
        state
            .queue
            .reserve(stamp(epoch, frame), model::Stream::Contact)
            .ok()
    });
    if ticket.is_none() {
        lose(frame);
    } else {
    }
    ticket
}

pub(super) fn actors(
    epoch: u64,
    frame: u64,
    objects: &[Option<super::contact_snapshot_model::Observation>; 3],
) -> [Option<model::Actor>; 2] {
    [0, 1].map(|i| {
        objects[if i == 0 && objects[0].is_some_and(|v| v.identity.category == 1) {
            2
        } else {
            i
        }]
        .and_then(|object| damage_capture::published_actor(epoch, frame, object.identity))
    })
}

pub(super) fn contact(ticket: model::Ticket, mut contact: model::Contact) {
    contact.raw.sample_index = ticket.index as u32;
    let stored = STATE
        .try_lock()
        .is_some_and(|mut state| state.queue.contact(ticket, contact).is_ok());
    if !stored {
        lose(ticket.stamp.frame);
    }
}

pub(super) fn cancel_update(call: commit::CallKey) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let Some(mut state) = STATE.try_lock() else {
        lose(call.frame);
        return;
    };
    for slot in &mut state.pending {
        if slot.is_some_and(|(old, _, _)| old == call) {
            *slot = None;
        }
    }
}

pub(super) fn begin_update(call: commit::CallKey, row: commit::Sample) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let actors = actors(call.epoch, call.frame, &row.objects);
    let Some(mut state) = STATE.try_lock() else {
        lose(call.frame);
        return;
    };
    let Some(index) = state.pending.iter().position(Option::is_none) else {
        state.queue.lose();
        return;
    };
    if let Ok(ticket) = state
        .queue
        .reserve(stamp(call.epoch, call.frame), model::Stream::Update)
    {
        state.pending[index] = Some((call, ticket, actors));
    } else {
        state.queue.lose();
    }
}

pub(super) fn complete_update(call: commit::CallKey, row: commit::Sample) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let after = actors(call.epoch, call.frame, &row.objects);
    let Some(mut state) = STATE.try_lock() else {
        lose(call.frame);
        return;
    };
    let Some(slot) = state
        .pending
        .iter_mut()
        .find(|slot| slot.is_some_and(|(old, _, _)| old == call))
    else {
        return;
    };
    let (_, ticket, before) = slot.take().unwrap();
    let actors = [0, 1].map(|i| before[i].filter(|actor| after[i] == Some(*actor)));
    let mut sample = row;
    sample.index = ticket.index as u32;
    if state
        .queue
        .update(ticket, model::Update { sample, actors })
        .is_err()
    {
        lose(call.frame);
    }
}

pub(super) fn on_frame(epoch: u64, frame: u64) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let delivery = stamp(epoch, frame);
    let Some(state) = STATE.try_lock() else {
        lose(frame);
        return;
    };
    let Some(capture) = state.queue.stamp() else {
        return;
    };
    if capture.frame == frame {
        return;
    }
    let counts = state.queue.counts();
    drop(state);
    // Fresh engine reads occur outside the queue lock. Model evaluation sees
    // only copied scalars, and the lifecycle fence is checked again per row.
    let mut current = [None; 8];
    if counts != [0, 0] {
        for record in crate::slight::agents::all_records()
            .iter()
            .filter(|record| record.category == 0)
        {
            if let Some(native) = unsafe { damage_capture::read_native_object(record.boid) } {
                if let Some(actor) = damage_capture::published_actor(epoch, frame, native.identity)
                {
                    current[actor.identity.entry as usize] = Some(actor);
                }
            }
        }
    }
    let Some(mut state) = STATE.try_lock() else {
        lose(frame);
        return;
    };
    if LOST_FRAME.load(Ordering::Acquire) >= capture.frame {
        state.queue.lose();
    }
    let status = state.queue.seal(stamp(epoch, frame));
    drop(state);
    if status.is_ok() {
        for _ in 0..model::CAPACITY {
            let decision = STATE.try_lock().map(|mut state| {
                state
                    .queue
                    .next(stamp(epoch, frame), |actor| current.contains(&Some(actor)))
            });
            let decision = match decision {
                Some(Ok(Some(decision))) => decision,
                Some(Ok(None)) => break,
                Some(Err(_)) => {
                    break;
                }
                None => {
                    lose(frame);
                    break;
                }
            };
            if stamp(epoch, frame) != delivery {
                lose(frame);
                break;
            }
            if LOST_FRAME.load(Ordering::Acquire) >= capture.frame {
                break;
            }
            match decision {
                model::Decision::Candidate(row) => {
                    super::live_behavior::register_damage_routes();
                    if super::damage_dispatch::handler_count(
                        super::damage_dispatch::Direction::Deal,
                    ) != 0
                        || super::damage_dispatch::handler_count(
                            super::damage_dispatch::Direction::Take,
                        ) != 0
                    {
                        super::damage_dispatch::deliver(row, delivery, || {
                            delivery_is_current(row, delivery)
                        });
                    }
                }
                model::Decision::Excluded { .. } => {}
            }
        }
    }
    if let Some(mut state) = STATE.try_lock() {
        state.pending.fill(None);
        let _ = state.queue.begin(stamp(epoch, frame));
    } else {
        lose(frame);
    }
}
