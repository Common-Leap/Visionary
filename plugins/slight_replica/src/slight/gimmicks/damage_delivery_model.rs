//! Ordered, single-attempt delivery of an already paired direct contact.
//! Payloads are explicit values; no ambient contact survives a callback.

use super::damage_feed_model::{Actor, Candidate, Stamp, CAPACITY};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Direction {
    Deal,
    Take,
    AttackShielded,
    ShieldHit,
}

/// Immutable root-hook input. A consumer cannot replace its amount, actor or
/// direction. Source and target are both retained for delivery validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    contact: Candidate,
    delivery: Stamp,
    direction: Direction,
}

impl Event {
    pub(super) fn contact(self) -> Candidate {
        self.contact
    }

    pub fn direction(self) -> Direction {
        self.direction
    }

    pub fn receiver(self) -> Actor {
        match self.direction {
            Direction::Deal | Direction::AttackShielded => self.contact.source,
            Direction::Take | Direction::ShieldHit => self.contact.target,
        }
    }

    pub fn source(self) -> Actor {
        self.contact.source
    }

    pub fn target(self) -> Actor {
        self.contact.target
    }

    /// Historical collision-admission status. The actor may have changed
    /// status by delivery without changing stock identity or invalidating
    /// this contact. Missing context is never converted to an idle status.
    pub fn receiver_status_at_contact(self) -> Option<i32> {
        match self.direction {
            Direction::Deal | Direction::AttackShielded => self.contact.source_status,
            Direction::Take | Direction::ShieldHit => self.contact.target_status,
        }
    }

    pub fn amount(self) -> Option<super::damage_commit_model::NativeDamageAmount> {
        self.contact.amount
    }

    pub fn stamp(self) -> Stamp {
        self.contact.stamp
    }

    pub fn delivery_stamp(self) -> Stamp {
        self.delivery
    }

    pub fn contact_index(self) -> usize {
        self.contact.contact_index
    }

    pub fn update_index(self) -> usize {
        self.contact.update_index
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    Reentrant,
    Lifetime,
    Late,
    Invalid,
    Consumed,
}

impl Rejection {
    pub fn name(self) -> &'static str {
        match self {
            Self::Reentrant => "reentrant",
            Self::Lifetime => "lifetime",
            Self::Late => "late",
            Self::Invalid => "invalid",
            Self::Consumed => "consumed",
        }
    }
}

/// A failed delivery is consumed too: a later callback cannot replay just the
/// defender side after an attacker handler invalidated either stock.
pub struct Cursor {
    last: Option<(Stamp, usize)>,
    active: bool,
}

impl Cursor {
    pub const fn new() -> Self {
        Self {
            last: None,
            active: false,
        }
    }

    pub fn begin(&mut self, contact: Candidate, delivery: Stamp) -> Result<[Event; 2], Rejection> {
        if self.active {
            return Err(Rejection::Reentrant);
        }
        let capture = contact.stamp;
        if capture.epoch != delivery.epoch || capture.revision != delivery.revision {
            return Err(Rejection::Lifetime);
        }
        if capture.frame.checked_add(1) != Some(delivery.frame) {
            return Err(Rejection::Late);
        }
        if (!contact.shield && contact.amount.is_none_or(|amount| amount.value() <= 0.0))
            || contact.weapon.is_some_and(|weapon| {
                !weapon.valid()
                    || weapon.category != 1
                    || weapon.founder != Some(contact.source.identity.object_id)
            })
            || contact.contact_index >= CAPACITY
            || contact.update_index >= CAPACITY
            || [contact.source_status, contact.target_status]
                .into_iter()
                .flatten()
                .any(|status| status < 0)
            || [contact.source, contact.target].iter().any(|actor| {
                !actor.identity.valid()
                    || actor.identity.category != 0
                    || actor.stock_generation == u32::MAX
            })
            || contact.source.identity.object_id == contact.target.identity.object_id
            || contact.source.identity.entry == contact.target.identity.entry
        {
            return Err(Rejection::Invalid);
        }
        if let Some((last, index)) = self.last {
            if (capture.epoch, capture.frame) < (last.epoch, last.frame)
                || (capture.epoch, capture.frame) == (last.epoch, last.frame)
                    && (capture.revision != last.revision || contact.contact_index <= index)
            {
                return Err(Rejection::Consumed);
            }
        }
        self.last = Some((capture, contact.contact_index));
        self.active = true;
        let directions = if contact.shield {
            [Direction::AttackShielded, Direction::ShieldHit]
        } else {
            [Direction::Deal, Direction::Take]
        };
        Ok(directions.map(|direction| Event {
            contact,
            delivery,
            direction,
        }))
    }

    pub fn finish(&mut self) {
        self.active = false;
    }
}
