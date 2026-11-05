//! Bounded per-contact shield queue. Sticky infliction flags are never events.
use super::contact_snapshot_model::Identity;
use super::damage_feed_model::{Actor, Candidate, Stamp, CAPACITY};

pub struct Queue {
    stamp: Option<Stamp>,
    rows: [Option<Candidate>; CAPACITY],
    count: usize,
    lost: bool,
}

impl Queue {
    pub const fn new() -> Self {
        Self {
            stamp: None,
            rows: [None; CAPACITY],
            count: 0,
            lost: false,
        }
    }

    pub fn push(
        &mut self,
        stamp: Stamp,
        actors: [Actor; 2],
        statuses: [Option<i32>; 2],
        weapon: Option<Identity>,
        blocked_power: Option<u32>,
    ) -> bool {
        if self.stamp != Some(stamp) {
            if self.stamp.is_some() {
                self.lost = true;
                return false;
            }
            self.stamp = Some(stamp);
        }
        if self.count == CAPACITY {
            self.lost = true;
            return false;
        }
        self.rows[self.count] = Some(Candidate {
            shield: true,
            stamp,
            contact_index: self.count,
            update_index: self.count,
            source: actors[0],
            target: actors[1],
            weapon,
            source_status: statuses[0],
            target_status: statuses[1],
            // Saved get_power result; no percent subtraction or shield HP inference.
            amount: blocked_power
                .and_then(super::damage_commit_model::NativeDamageAmount::blocked_power),
            before_bits: 0,
            after_bits: 0,
            cap_bits: 0,
        });
        self.count += 1;
        true
    }

    pub fn drain(&mut self, delivery: Stamp) -> [Option<Candidate>; CAPACITY] {
        let old = std::mem::replace(self, Self::new());
        if old.lost
            || old.stamp.is_none_or(|stamp| {
                stamp.epoch != delivery.epoch
                    || stamp.revision != delivery.revision
                    || stamp.frame.checked_add(1) != Some(delivery.frame)
            })
        {
            [None; CAPACITY]
        } else {
            old.rows
        }
    }
}
