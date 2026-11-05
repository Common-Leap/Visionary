//! Typed direct-contact delivery, separate from legacy object-only firings.
//! Registration alone does not enable public graph/export damage support.

use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::LazyLock;

use super::damage_delivery_model::{Cursor, Rejection};
pub use super::damage_delivery_model::{Direction, Event};
use super::damage_feed_model::{Candidate, Stamp};

pub type Handler = fn(Event);
pub type HandlerId = u64;
static CONSUMERS: AtomicUsize = AtomicUsize::new(0);

/// Cheap demand signal for deferred native installation. No engine hook is
/// installed by registration itself, including registration on the TCP thread.
pub(super) fn has_consumers() -> bool {
    CONSUMERS.load(Ordering::Acquire) != 0
}

struct State {
    next: HandlerId,
    generation: u64,
    delivering: bool,
    handlers: BTreeMap<Direction, Vec<(HandlerId, Handler)>>,
    cursor: Cursor,
    shield_cursor: Cursor,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| {
    Mutex::new(State {
        next: 1,
        generation: 0,
        delivering: false,
        handlers: BTreeMap::new(),
        cursor: Cursor::new(),
        shield_cursor: Cursor::new(),
    })
});

/// The same direction/function pair is registered once across reconnects.
pub fn register(direction: Direction, handler: Handler) -> HandlerId {
    let mut state = STATE.lock();
    if let Some((id, _)) = state
        .handlers
        .get(&direction)
        .into_iter()
        .flatten()
        .find(|(_, known)| *known as usize == handler as usize)
    {
        return *id;
    }
    let id = state.next;
    state.next = state
        .next
        .checked_add(1)
        .expect("damage handler IDs exhausted");
    state
        .handlers
        .entry(direction)
        .or_default()
        .push((id, handler));
    CONSUMERS.fetch_add(1, Ordering::Release);
    id
}

pub fn unregister(id: HandlerId) -> bool {
    let mut state = STATE.lock();
    let mut removed = false;
    for handlers in state.handlers.values_mut() {
        let before = handlers.len();
        handlers.retain(|(known, _)| *known != id);
        removed |= before != handlers.len();
        CONSUMERS.fetch_sub(before - handlers.len(), Ordering::Release);
    }
    removed
}

pub fn handler_count(direction: Direction) -> usize {
    STATE.lock().handlers.get(&direction).map_or(0, Vec::len)
}

pub fn clear() {
    let mut state = STATE.lock();
    state.handlers.clear();
    CONSUMERS.store(0, Ordering::Release);
    state.generation = state.generation.wrapping_add(1);
    // Keep the cursor, including its active fence. Clearing consumers must
    // neither permit replay in this frame nor unlock a nested delivery.
}

struct DeliveryGuard;

impl Drop for DeliveryGuard {
    fn drop(&mut self) {
        let mut state = STATE.lock();
        state.delivering = false;
        state.cursor.finish();
        state.shield_cursor.finish();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Delivered { handlers: usize },
    Invalidated { handlers: usize },
    Rejected(Rejection),
}

impl Outcome {
    pub(super) fn summary(self) -> (&'static str, usize) {
        match self {
            Self::Delivered { handlers } => ("delivered", handlers),
            Self::Invalidated { handlers } => ("invalidated", handlers),
            Self::Rejected(reason) => (reason.name(), 0),
        }
    }
}

/// Called synchronously by the sealed native feed. The validation closure
/// freshly resolves BOTH actors and its frame/lifecycle fence each time.
/// No engine operation or consumer callback runs under the registry lock.
pub(super) fn deliver(
    contact: Candidate,
    delivery: Stamp,
    mut current: impl FnMut() -> bool,
) -> Outcome {
    let (events, handlers, generation) = {
        let mut state = STATE.lock();
        if state.delivering {
            return Outcome::Rejected(Rejection::Reentrant);
        }
        let cursor = if contact.shield {
            &mut state.shield_cursor
        } else {
            &mut state.cursor
        };
        let events = match cursor.begin(contact, delivery) {
            Ok(events) => events,
            Err(reason) => return Outcome::Rejected(reason),
        };
        state.delivering = true;
        let handlers = events.map(|event| {
            state
                .handlers
                .get(&event.direction())
                .map(|rows| rows.iter().map(|(_, handler)| *handler).collect::<Vec<_>>())
                .unwrap_or_default()
        });
        (events, handlers, state.generation)
    };
    let _guard = DeliveryGuard;
    let mut delivered = 0;
    for (event, handlers) in events.into_iter().zip(handlers) {
        // Validate even the empty direction: an absent attacker consumer must
        // not let an invalidated contact reach the defender consumer.
        if !current() || STATE.lock().generation != generation {
            return Outcome::Invalidated {
                handlers: delivered,
            };
        }
        for handler in handlers {
            if !current() || STATE.lock().generation != generation {
                return Outcome::Invalidated {
                    handlers: delivered,
                };
            }
            handler(event);
            delivered += 1;
        }
    }
    // Detect a final handler that changes stock or clears the dispatcher too.
    if !current() || STATE.lock().generation != generation {
        return Outcome::Invalidated {
            handlers: delivered,
        };
    }
    Outcome::Delivered {
        handlers: delivered,
    }
}
