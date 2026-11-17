//! Exact live fighter identity shared by dispatch and immediate native reads.

use super::registry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    pub fighter_kind: i32,
    pub entry_id: i32,
    pub module_accessor_addr: u64,
    pub stock_generation: u32,
}

impl Identity {
    pub(crate) fn from_live_fighter(boid: u32, fighter: &str) -> Option<Self> {
        let record = crate::slight::agents::lookup(boid)?;
        if record.category != 0 || record.module_accessor_addr == 0 || record.entry_id < 0 {
            return None;
        }
        // A cached agent record is not enough: BOIDs and entries can be reused.
        unsafe {
            if smash::app::sv_battle_object::is_null(boid)
                || !smash::app::sv_battle_object::is_active(boid)
            {
                return None;
            }
            let accessor = smash::app::sv_battle_object::module_accessor(boid);
            if accessor.is_null()
                || accessor as u64 != record.module_accessor_addr
                || smash::app::sv_battle_object::category(boid) != 0
                || smash::app::sv_battle_object::kind(boid) != record.kind
                || smash::app::sv_battle_object::entry_id(boid) != record.entry_id
            {
                return None;
            }
        }
        if registry::fighter_name(record.kind) != Some(fighter) {
            return None;
        }
        Some(Self {
            fighter_kind: record.kind,
            entry_id: record.entry_id,
            module_accessor_addr: record.module_accessor_addr,
            stock_generation: crate::slight::agents::stock_generation(&record)?,
        })
    }

    pub(crate) fn state_identity(self) -> super::state::ActorIdentity {
        super::state::ActorIdentity {
            fighter_kind: self.fighter_kind,
            entry_id: self.entry_id,
            module_accessor_addr: self.module_accessor_addr,
            stock_generation: self.stock_generation,
        }
    }
}
