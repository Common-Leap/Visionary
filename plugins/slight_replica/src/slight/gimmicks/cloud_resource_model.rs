//! Closed Cloud resource operations and their native postconditions.
//!
//! These operations augment the native resource. They do not claim ownership
//! of its default gain, countdown, spend, mobility or HUD lifecycle.

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Command {
    Adjust(f32),
    Clear,
}

impl Command {
    pub(crate) fn validate(self) -> Result<(), &'static str> {
        if let Self::Adjust(delta) = self {
            if !delta.is_finite() || delta.abs() > 1_000_000.0 {
                return Err("Limit adjustment must be finite and within one million units");
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Snapshot {
    pub gauge: f32,
    pub remaining: i32,
    pub full: bool,
    pub set_custom: bool,
    pub special: bool,
    pub previous_gauge: f32,
    pub status: i32,
    pub changing: bool,
}

impl Snapshot {
    pub(crate) fn validate(self) -> Result<(), &'static str> {
        if !self.gauge.is_finite()
            || !(0.0..=100.0).contains(&self.gauge)
            || !self.previous_gauge.is_finite()
            || !(0.0..=100.0).contains(&self.previous_gauge)
            || self.remaining < 0
            || (self.full && self.gauge != 100.0)
        {
            return Err("native Limit snapshot is outside the reviewed scalar contract");
        }
        // The game raises Limit on hits during any of Cloud's attacks, so only a status change
        // in progress is refused.
        if self.changing {
            return Err("native Limit writes wait for a status change to finish");
        }
        Ok(())
    }
}

pub(crate) fn preflight(command: Command, before: Snapshot) -> Result<(), &'static str> {
    command.validate()?;
    before.validate()
}

pub(crate) fn verify(
    command: Command,
    before: Snapshot,
    after: Snapshot,
) -> Result<(), &'static str> {
    preflight(command, before)?;
    after.validate()?;
    if after.status != before.status {
        return Err("native Limit status changed during the operation");
    }
    match command {
        Command::Adjust(_) if before.full => {
            if before != after {
                return Err("an adjustment changed an already-full native Limit resource");
            }
        }
        Command::Adjust(delta) => {
            let expected = (before.gauge + delta).clamp(0.0, 100.0);
            if after.gauge != expected
                || after.full != (expected == 100.0)
                || (after.full && !after.set_custom)
                || (!after.full && after.remaining != before.remaining)
                || (!after.full && after.set_custom != before.set_custom)
                || after.special != before.special
                || after.previous_gauge != before.previous_gauge
            {
                return Err("native Limit adjustment postcondition failed");
            }
        }
        Command::Clear => {
            if after.gauge != 0.0
                || after.remaining != 0
                || after.full
                || after.set_custom
                || after.special
                || after.previous_gauge != 0.0
            {
                return Err("native Limit cleanup/reset postcondition failed");
            }
        }
    }
    Ok(())
}
