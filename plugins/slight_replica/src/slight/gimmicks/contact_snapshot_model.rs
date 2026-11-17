//! Scalar native contact observations. No pointer is retained for later dereference.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub object_id: u32,
    pub accessor: u64,
    pub category: i32,
    pub kind: i32,
    pub entry: i32,
    pub founder: Option<u32>,
}

impl Identity {
    pub fn valid(self) -> bool {
        self.accessor != 0
            && self.accessor.is_multiple_of(8)
            && self.kind >= 0
            && matches!(self.category, 0 | 1)
            && self.object_id >> 28 == self.category as u32
            && (self.category != 0 || (0..8).contains(&self.entry))
            && (self.category != 0 || self.founder.is_none())
            && self.founder.is_none_or(|id| id >> 28 == 0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    pub identity: Identity,
    pub percent_bits: Option<u32>,
}

/// Admit a scalar only between matching immediate native identity reads.
/// This does not assign an attacker role or turn a percent into a damage event.
pub fn coherent(before: Identity, after: Identity, percent: Option<f32>) -> Option<Observation> {
    if before != after || !before.valid() {
        return None;
    }
    let percent_bits = match (before.category, percent) {
        (0, Some(value)) if value.is_finite() && (0.0..=1_000_000.0).contains(&value) => {
            Some(value.to_bits())
        }
        (1, None) => None,
        _ => return None,
    };
    Some(Observation {
        identity: before,
        percent_bits,
    })
}

/// Only direct, unchanged fighter ownership grants damage credit. A founder
/// alone cannot establish the current owner of a reflected/transferred weapon.
pub fn fighter_owner(founder: i32, link_owner: i32) -> Option<u32> {
    (founder >= 0 && founder == link_owner && (founder as u32) >> 28 == 0).then_some(founder as u32)
}
