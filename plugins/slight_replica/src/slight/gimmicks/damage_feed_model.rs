//! Bounded closed-frame contact pairing. Candidates are not gameplay events.
//! No allocation, native calls, or historical pointer dereferences occur here.

use super::{
    contact_snapshot_model as native, damage_capture_model::RawSample,
    damage_commit_model as commit,
};

pub const CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub epoch: u64,
    pub frame: u64,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Actor {
    pub identity: native::Identity,
    pub stock_generation: u32,
}

impl Actor {
    fn valid(self) -> bool {
        self.identity.valid() && self.identity.category == 0 && self.stock_generation != u32::MAX
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contact {
    pub raw: RawSample,
    pub objects: [Option<native::Observation>; 3],
    pub actors: [Option<Actor>; 2],
    /// Status copied during each participant's immediate lifetime admission.
    /// It is historical context, separate from stock/identity equality.
    pub statuses: [Option<i32>; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Update {
    pub sample: commit::Sample,
    pub actors: [Option<Actor>; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Contact,
    Update,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub stamp: Stamp,
    pub stream: Stream,
    pub index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    NotRecording,
    StaleTicket,
    Capacity,
    Duplicate,
    Incomplete,
    CaptureLost,
    LifetimeChanged,
    LateDelivery,
    NotClosed,
}

impl Fault {
    pub fn name(self) -> &'static str {
        match self {
            Self::NotRecording => "not_recording",
            Self::StaleTicket => "stale_ticket",
            Self::Capacity => "capacity",
            Self::Duplicate => "duplicate",
            Self::Incomplete => "incomplete",
            Self::CaptureLost => "capture_lost",
            Self::LifetimeChanged => "lifetime_changed",
            Self::LateDelivery => "late_delivery",
            Self::NotClosed => "not_closed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exclusion {
    Caller,
    Item,
    WeaponCredit,
    Unsupported,
    SelfContact,
    Unmatched,
    Ambiguous,
    Order,
    Lifetime,
}

impl Exclusion {
    pub fn name(self) -> &'static str {
        match self {
            Self::Caller => "caller",
            Self::Item => "item",
            Self::WeaponCredit => "weapon_credit",
            Self::Unsupported => "unsupported",
            Self::SelfContact => "self_contact",
            Self::Unmatched => "unmatched",
            Self::Ambiguous => "ambiguous",
            Self::Order => "order",
            Self::Lifetime => "lifetime",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub shield: bool,
    pub stamp: Stamp,
    pub contact_index: usize,
    pub update_index: usize,
    pub source: Actor,
    pub target: Actor,
    /// Original weapon identity, retained only for fresh owner/lifetime proof.
    pub weapon: Option<native::Identity>,
    pub source_status: Option<i32>,
    pub target_status: Option<i32>,
    pub before_bits: u32,
    pub amount: Option<commit::NativeDamageAmount>,
    pub cap_bits: u32,
    pub after_bits: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Candidate(Candidate),
    Excluded { index: usize, reason: Exclusion },
}

pub struct Queue {
    stamp: Option<Stamp>,
    sealed: bool,
    fault: Option<Fault>,
    claimed: [usize; 2],
    contacts: [Option<Contact>; CAPACITY],
    updates: [Option<Update>; CAPACITY],
    cursor: usize,
    pairs: [Option<usize>; CAPACITY],
    ambiguous: [bool; CAPACITY],
    reversed: bool,
}

impl Queue {
    pub const fn new() -> Self {
        Self {
            stamp: None,
            sealed: false,
            fault: None,
            claimed: [0; 2],
            contacts: [None; CAPACITY],
            updates: [None; CAPACITY],
            cursor: 0,
            pairs: [None; CAPACITY],
            ambiguous: [false; CAPACITY],
            reversed: false,
        }
    }

    /// The caller explicitly discards any old frame before starting another.
    /// Old reservations cannot populate this frame because tickets carry its stamp.
    pub fn begin(&mut self, stamp: Stamp) -> Result<(), Fault> {
        if self.stamp.is_some_and(|old| {
            stamp.epoch < old.epoch
                || (stamp.epoch == old.epoch && stamp.frame <= old.frame)
                || stamp.revision < old.revision
        }) {
            return Err(Fault::StaleTicket);
        }
        self.stamp = Some(stamp);
        self.sealed = false;
        self.fault = None;
        self.claimed = [0; 2];
        self.contacts.fill(None);
        self.updates.fill(None);
        self.cursor = 0;
        self.pairs.fill(None);
        self.ambiguous.fill(false);
        self.reversed = false;
        Ok(())
    }

    pub fn counts(&self) -> [usize; 2] {
        self.claimed
    }
    pub fn stamp(&self) -> Option<Stamp> {
        self.stamp
    }

    pub fn lose(&mut self) {
        self.fault.get_or_insert(Fault::CaptureLost);
    }

    pub fn reserve(&mut self, stamp: Stamp, stream: Stream) -> Result<Ticket, Fault> {
        if self.stamp != Some(stamp) {
            return Err(Fault::StaleTicket);
        }
        if self.sealed {
            return Err(Fault::NotRecording);
        }
        if let Some(fault) = self.fault {
            return Err(fault);
        }
        let claimed = &mut self.claimed[usize::from(stream == Stream::Update)];
        if *claimed == CAPACITY {
            self.fault = Some(Fault::Capacity);
            return Err(Fault::Capacity);
        }
        let ticket = Ticket {
            stamp,
            stream,
            index: *claimed,
        };
        *claimed += 1;
        Ok(ticket)
    }

    fn check(
        &mut self,
        ticket: Ticket,
        stream: Stream,
        epoch: u64,
        frame: u64,
    ) -> Result<(), Fault> {
        if self.stamp != Some(ticket.stamp) {
            return Err(Fault::StaleTicket);
        }
        if self.sealed {
            return Err(Fault::NotRecording);
        }
        if let Some(fault) = self.fault {
            return Err(fault);
        }
        if ticket.stream != stream
            || ticket.index >= self.claimed[usize::from(stream == Stream::Update)]
            || ticket.stamp.epoch != epoch
            || ticket.stamp.frame != frame
        {
            self.lose();
            return Err(Fault::StaleTicket);
        }
        Ok(())
    }

    pub fn contact(&mut self, ticket: Ticket, contact: Contact) -> Result<(), Fault> {
        self.check(
            ticket,
            Stream::Contact,
            contact.raw.epoch,
            contact.raw.frame,
        )?;
        if self.contacts[ticket.index].is_some() {
            self.fault = Some(Fault::Duplicate);
            return Err(Fault::Duplicate);
        }
        self.contacts[ticket.index] = Some(contact);
        Ok(())
    }

    pub fn update(&mut self, ticket: Ticket, update: Update) -> Result<(), Fault> {
        self.check(
            ticket,
            Stream::Update,
            update.sample.epoch,
            update.sample.frame,
        )?;
        if self.updates[ticket.index].is_some() {
            self.fault = Some(Fault::Duplicate);
            return Err(Fault::Duplicate);
        }
        self.updates[ticket.index] = Some(update);
        Ok(())
    }

    fn delivery_is_current(&self, delivery: Stamp) -> Result<(), Fault> {
        let stamp = self.stamp.ok_or(Fault::NotRecording)?;
        if stamp.epoch != delivery.epoch || stamp.revision != delivery.revision {
            return Err(Fault::LifetimeChanged);
        }
        if stamp.frame.checked_add(1) != Some(delivery.frame) {
            return Err(Fault::LateDelivery);
        }
        Ok(())
    }

    pub fn seal(&mut self, delivery: Stamp) -> Result<(), Fault> {
        if self.sealed {
            return Err(Fault::NotRecording);
        }
        if let Some(stamp) = self.stamp {
            if stamp.epoch == delivery.epoch
                && stamp.revision == delivery.revision
                && stamp.frame == delivery.frame
            {
                return Err(Fault::NotClosed);
            }
        }
        if let Err(fault) = self.delivery_is_current(delivery) {
            self.fault = Some(fault);
        }
        self.sealed = true;
        if let Some(fault) = self.fault {
            return Err(fault);
        }
        if self.contacts[..self.claimed[0]].iter().any(Option::is_none)
            || self.updates[..self.claimed[1]].iter().any(Option::is_none)
        {
            self.fault = Some(Fault::Incomplete);
            return Err(Fault::Incomplete);
        }
        let mut uses = [0usize; CAPACITY];
        for i in 0..self.claimed[0] {
            for (j, count) in uses.iter_mut().enumerate().take(self.claimed[1]) {
                if matches(
                    self.contacts[i].as_ref().unwrap(),
                    self.updates[j].as_ref().unwrap(),
                ) {
                    self.ambiguous[i] |= self.pairs[i].is_some();
                    self.pairs[i] = Some(j);
                    *count += 1;
                }
            }
        }
        let mut last = None;
        for i in 0..self.claimed[0] {
            if let Some(j) = self.pairs[i] {
                self.ambiguous[i] |= uses[j] != 1;
                if !self.ambiguous[i] {
                    self.reversed |= last.is_some_and(|previous| previous >= j);
                    last = Some(j);
                }
            }
        }
        Ok(())
    }

    pub fn next(
        &mut self,
        delivery: Stamp,
        mut current: impl FnMut(Actor) -> bool,
    ) -> Result<Option<Decision>, Fault> {
        if !self.sealed {
            return Err(Fault::NotClosed);
        }
        if let Some(fault) = self.fault {
            return Err(fault);
        }
        if let Err(fault) = self.delivery_is_current(delivery) {
            self.fault = Some(fault);
            return Err(fault);
        }
        let i = self.cursor;
        if i == self.claimed[0] {
            return Ok(None);
        }
        self.cursor += 1;
        let contact = self.contacts[i].unwrap();
        let excluded = |reason| Ok(Some(Decision::Excluded { index: i, reason }));
        if contact.raw.caller_return_offset != Some(0x6c2f10) {
            return excluded(Exclusion::Caller);
        }
        match contact.raw.object_id_1 >> 28 {
            4 => return excluded(Exclusion::Item),
            1 => {
                if !contact.objects[0].is_some_and(|v| {
                    v.identity.valid()
                        && v.identity.category == 1
                        && v.identity.object_id == contact.raw.object_id_1
                }) {
                    return excluded(Exclusion::WeaponCredit);
                }
            }
            0 => {}
            _ => return excluded(Exclusion::Unsupported),
        }
        if contact.raw.object_id_1 == contact.raw.object_id_2 {
            return excluded(Exclusion::SelfContact);
        }
        if self.reversed {
            return excluded(Exclusion::Order);
        }
        if self.ambiguous[i] {
            return excluded(Exclusion::Ambiguous);
        }
        let Some(j) = self.pairs[i] else {
            return excluded(Exclusion::Unmatched);
        };
        let update = self.updates[j].unwrap();
        let [Some(source), Some(target)] = contact.actors else {
            return excluded(Exclusion::Lifetime);
        };
        let source_object = contact.objects[0].map(|v| v.identity);
        let weapon = source_object.filter(|v| v.category == 1);
        let credited = if let Some(weapon) = weapon {
            if !weapon.valid()
                || weapon.founder != Some(source.identity.object_id)
                || contact.objects[2].map(|v| v.identity) != Some(source.identity)
                || update.sample.objects[2].map(|v| v.identity) != Some(source.identity)
            {
                return excluded(Exclusion::WeaponCredit);
            }
            Some(source.identity)
        } else {
            source_object
        };
        if source.identity.object_id == target.identity.object_id
            || source.identity.entry == target.identity.entry
        {
            return excluded(Exclusion::SelfContact);
        }
        if !source.valid()
            || !target.valid()
            || update.actors != contact.actors
            || Some(source.identity) != credited
            || Some(target.identity) != contact.objects[1].map(|v| v.identity)
            || !current(source)
            || !current(target)
            || !current(source)
        {
            return excluded(Exclusion::Lifetime);
        }
        Ok(Some(Decision::Candidate(Candidate {
            shield: false,
            stamp: self.stamp.unwrap(),
            contact_index: i,
            update_index: j,
            source,
            target,
            weapon,
            source_status: contact.statuses[0].filter(|status| *status >= 0),
            target_status: contact.statuses[1].filter(|status| *status >= 0),
            before_bits: update.sample.before_bits,
            amount: commit::confirmed_pending(&update.sample),
            cap_bits: update.sample.cap_bits.unwrap(),
            after_bits: update.sample.after_bits,
        })))
    }
}

fn matches(contact: &Contact, update: &Update) -> bool {
    let raw = contact.raw;
    let sample = update.sample;
    raw.caller_return_offset == Some(0x6c2f10)
        && raw.epoch == sample.epoch
        && raw.frame == sample.frame
        && raw.object_id_1 == sample.context_id
        && raw.amount_bits == sample.pending_bits
        && commit::confirmed_pending(&sample).is_some()
        && contact.objects[1].is_some_and(|before| {
            before.identity.valid()
                && before.identity.category == 0
                && before.identity.object_id == raw.object_id_2
                && before.percent_bits == Some(sample.before_bits)
                && Some(before.identity) == sample.objects[1].map(|v| v.identity)
        })
        && contact.objects[0].is_some_and(|source| {
            source.identity.valid()
                && source.identity.object_id == raw.object_id_1
                && Some(source.identity) == sample.objects[0].map(|v| v.identity)
        })
}
