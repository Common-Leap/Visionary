//! Fixed-capacity, one-frame receiver contact buffer. Overflow drops the batch.
use super::{
    damage_feed_model::{Actor, Stamp, CAPACITY},
    native_inputs::DamageInput,
};

#[derive(Clone, Copy)]
pub struct Contact {
    pub actor: Actor,
    pub stamp: Stamp,
    pub input: DamageInput,
}

pub struct Queue {
    rows: [Option<Contact>; CAPACITY],
    count: usize,
    lost: bool,
}
impl Queue {
    pub const fn new() -> Self {
        Self {
            rows: [None; CAPACITY],
            count: 0,
            lost: false,
        }
    }
    pub fn push(&mut self, contact: Contact) {
        if self.count == CAPACITY {
            self.lost = true;
            return;
        }
        self.rows[self.count] = Some(contact);
        self.count += 1;
    }
    pub fn drain(&mut self, stamp: Stamp) -> [Option<Contact>; CAPACITY] {
        let old = std::mem::replace(self, Self::new());
        if old.lost
            || old.rows.iter().flatten().any(|r| {
                r.stamp.epoch != stamp.epoch
                    || r.stamp.revision != stamp.revision
                    || r.stamp.frame.checked_add(1) != Some(stamp.frame)
            })
        {
            [None; CAPACITY]
        } else {
            old.rows
        }
    }
}
