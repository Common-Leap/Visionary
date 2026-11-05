//! Bounded observations after the pinned native percent-add branch.
//! Arithmetic agreement is diagnostic evidence, not attacker attribution.

use super::contact_snapshot_model::Observation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub epoch: u64,
    pub frame: u64,
    pub index: u32,
    pub context_id: u32,
    pub caller_return_offset: Option<u64>,
    pub before_bits: u32,
    pub pending_bits: u32,
    pub after_bits: u32,
    pub cap_bits: Option<u32>,
    pub suppressed: bool,
    pub objects: [Option<Observation>; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agreement {
    Unknown,
    Suppressed,
    Unchanged,
    Added,
    Clamped,
    Rounded,
    Mismatch,
}

impl Agreement {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Suppressed => "suppressed",
            Self::Unchanged => "unchanged",
            Self::Added => "added",
            Self::Clamped => "clamped",
            Self::Rounded => "rounded",
            Self::Mismatch => "mismatch",
        }
    }
}

pub fn agreement(sample: &Sample) -> Agreement {
    let Some(target) = sample.objects[1] else {
        return Agreement::Unknown;
    };
    if !target.identity.valid()
        || target.identity.category != 0
        || target.percent_bits != Some(sample.after_bits)
    {
        return Agreement::Mismatch;
    }
    let before = f32::from_bits(sample.before_bits);
    let pending = f32::from_bits(sample.pending_bits);
    let after = f32::from_bits(sample.after_bits);
    if !before.is_finite()
        || !pending.is_finite()
        || !after.is_finite()
        || !(0.0..=1_000_000.0).contains(&before)
        || !(0.0..=1_000_000.0).contains(&after)
    {
        return Agreement::Mismatch;
    }
    if sample.suppressed {
        return if before == after {
            Agreement::Suppressed
        } else {
            Agreement::Mismatch
        };
    }
    let sum = before + pending;
    if let Some(bits) = sample.cap_bits {
        let cap = f32::from_bits(bits);
        if !cap.is_finite()
            || !(before..=1_000_000.0).contains(&cap)
            || !sum.is_finite()
            || sum.min(cap).to_bits() != sample.after_bits
        {
            return Agreement::Mismatch;
        }
        if pending > 0.0 {
            return if sum > cap {
                Agreement::Clamped
            } else if after > before {
                Agreement::Added
            } else if sum.to_bits() == sample.before_bits {
                Agreement::Rounded
            } else {
                Agreement::Mismatch
            };
        }
    }
    if (before + pending).to_bits() != sample.after_bits {
        return Agreement::Mismatch;
    }
    if pending > 0.0 && after > before {
        Agreement::Added
    } else if pending == 0.0 && after == before {
        Agreement::Unchanged
    } else {
        Agreement::Mismatch
    }
}

/// A confirmed native contact amount, never post-minus-pre percent.
/// Damage uses the percent-commit caller; shield contacts use the saved
/// AttackModule::get_power result (blocked power, not shield-HP depletion).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeDamageAmount(u32);

impl NativeDamageAmount {
    #[allow(dead_code)] // Shield feed; the percent-only harness omits that producer.
    pub(super) fn blocked_power(bits: u32) -> Option<Self> {
        let power = f32::from_bits(bits);
        (power.is_finite() && (0.0..=1_000_000.0).contains(&power)).then_some(Self(bits))
    }

    pub fn bits(self) -> u32 {
        self.0
    }
    pub fn value(self) -> f32 {
        f32::from_bits(self.0)
    }
}

pub fn confirmed_pending(sample: &Sample) -> Option<NativeDamageAmount> {
    (sample.caller_return_offset == Some(0x3febd8)
        && sample.cap_bits.is_some()
        && f32::from_bits(sample.pending_bits).is_finite()
        && f32::from_bits(sample.pending_bits) > 0.0
        && f32::from_bits(sample.pending_bits) <= 1_000_000.0
        && matches!(
            agreement(sample),
            Agreement::Added | Agreement::Clamped | Agreement::Rounded
        ))
    .then_some(NativeDamageAmount(sample.pending_bits))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallKey {
    pub epoch: u64,
    pub frame: u64,
    pub stack: u64,
    pub module: u64,
    pub data: u64,
}

/// Keys are compared as scalar numbers, never dereferenced after capture.
pub struct PendingStore {
    calls: [Option<(CallKey, Sample)>; 16],
}

impl PendingStore {
    pub const fn new() -> Self {
        Self { calls: [None; 16] }
    }

    pub fn expire(&mut self, epoch: u64, frame: u64) {
        for call in &mut self.calls {
            if call.is_some_and(|(key, _)| key.epoch != epoch || key.frame != frame) {
                *call = None;
            }
        }
    }

    pub fn begin(&mut self, key: CallKey, sample: Sample) -> bool {
        if key.epoch != sample.epoch
            || key.frame != sample.frame
            || key.stack == 0
            || !key.stack.is_multiple_of(16)
            || key.module == 0
            || key.data == 0
        {
            return false;
        }
        self.expire(key.epoch, key.frame);
        // A missed end may leave this exact call slot occupied. Replacing the
        // reservation preserves a visible gap rather than borrowing its data.
        let matching = self
            .calls
            .iter()
            .position(|call| call.is_some_and(|(old, _)| old == key));
        let Some(index) = matching.or_else(|| self.calls.iter().position(Option::is_none)) else {
            return false;
        };
        self.calls[index] = Some((key, sample));
        true
    }

    pub fn end(&mut self, key: CallKey) -> Option<Sample> {
        let call = self
            .calls
            .iter_mut()
            .find(|call| call.is_some_and(|(old, _)| old == key))?;
        call.take().map(|(_, sample)| sample)
    }
}
