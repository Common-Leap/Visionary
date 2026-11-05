//! Closed capture-target commands. A queued command never follows a new target.
use super::{damage_commands::Lease, phase_commands::Command};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target<I> {
    pub boid: u32,
    pub identity: I,
}

#[derive(Clone, Copy, Debug)]
pub struct Pending<I> {
    pub owner: Target<I>,
    pub target: Target<I>,
    pub lease: Lease,
    pub command: Command,
}

impl<I: Copy + Eq> Pending<I> {
    /// Resolve both actors and the current link again at delivery. The guard
    /// runs again after native reads, immediately before the native write.
    pub fn check(
        &self,
        owner: Target<I>,
        target: Option<Target<I>>,
        lease: Lease,
    ) -> Result<(), String> {
        if self.owner != owner || self.lease != lease {
            return Err("grab command owner, status or configuration changed".into());
        }
        if target != Some(self.target) {
            return Err("held opponent disappeared or capture link changed".into());
        }
        Ok(())
    }
}

/// A chart-owned status must not hold longer than this, however the chart is written.
pub const MAX_OWNED_HOLD_FRAMES: u32 = 600;
/// Budget for each stretch outside owned statuses, e.g. a vanilla throw. Longer than any
/// vanilla throw, because a throw that never finishes has no other way out.
pub const MAX_UNOWNED_HOLD_FRAMES: u32 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseReason {
    OwnerHit,
    TargetLost,
    Timeout,
}
impl ReleaseReason {
    pub fn name(self) -> &'static str {
        match self {
            Self::OwnerHit => "owner_hit",
            Self::TargetLost => "target_lost",
            Self::Timeout => "timeout",
        }
    }
}

/// A capture first seen in a chart-owned status, followed until both fighters are free.
/// Captures that never touch an owned status have no row, so vanilla grabs are untouched.
#[derive(Clone, Copy, Debug)]
pub struct OwnedHold<I> {
    pub target: Target<I>,
    pub owner_damage: f32,
    pub frames: u32,
    pub owned: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct HoldSample<I> {
    pub owned: bool,
    pub held: Option<Target<I>>,
    pub owner_damage: f32,
    pub target_alive: bool,
    /// The owner is in a catch or throw status, or the target is still in a capture status.
    pub engaged: bool,
}

/// One owning-Main observation. Armored hits raise damage without changing status,
/// so a damage rise is the hit signal; status changes are handled at status exit.
pub fn step_owned_hold<I: Copy + Eq>(
    row: Option<OwnedHold<I>>,
    sample: HoldSample<I>,
) -> (Option<OwnedHold<I>>, Option<ReleaseReason>) {
    let HoldSample {
        owned,
        held,
        owner_damage,
        target_alive,
        engaged,
    } = sample;
    if !owned {
        let Some(row) = row.filter(|_| engaged) else {
            return (None, None);
        };
        let frames = if row.owned { 0 } else { row.frames + 1 };
        // The row survives a rescue: if the rescue did not free them, the next one runs.
        return if frames >= MAX_UNOWNED_HOLD_FRAMES {
            (
                Some(OwnedHold {
                    frames: 0,
                    owned: false,
                    ..row
                }),
                Some(ReleaseReason::Timeout),
            )
        } else {
            (
                Some(OwnedHold {
                    frames,
                    owned: false,
                    ..row
                }),
                None,
            )
        };
    }
    let Some(target) = held else {
        return (None, row.map(|_| ReleaseReason::TargetLost));
    };
    let Some(row) = row.filter(|row| row.owned && row.target == target) else {
        let fresh = OwnedHold {
            target,
            owner_damage,
            frames: 0,
            owned: true,
        };
        return (
            Some(fresh),
            (!target_alive).then_some(ReleaseReason::TargetLost),
        );
    };
    let reason = if !target_alive {
        Some(ReleaseReason::TargetLost)
    } else if owner_damage > row.owner_damage {
        Some(ReleaseReason::OwnerHit)
    } else if row.frames + 1 >= MAX_OWNED_HOLD_FRAMES {
        Some(ReleaseReason::Timeout)
    } else {
        None
    };
    let next = OwnedHold {
        target,
        owner_damage,
        frames: row.frames + 1,
        owned: true,
    };
    (reason.is_none().then_some(next), reason)
}

/// Leaving an owned status keeps the capture only for a status that manages it. The row
/// starts here at the latest: a capture can form and leave the owned status within one
/// frame, before any Main-line observation sees it held there.
pub fn exit_owned_status<I: Copy + Eq>(
    row: Option<OwnedHold<I>>,
    held: Option<Target<I>>,
    owner_damage: f32,
    next: i32,
    next_owned: bool,
    catch_family: &[i32],
) -> (Option<OwnedHold<I>>, bool) {
    let row = match (row, held) {
        (Some(row), Some(target)) if row.target == target => Some(row),
        (_, Some(target)) => Some(OwnedHold {
            target,
            owner_damage,
            frames: 0,
            owned: true,
        }),
        (row, None) => row,
    };
    (row, next_owned || catch_family.contains(&next))
}

pub struct Queue<I> {
    slots: [Vec<Pending<I>>; 8],
}
impl<I> Default for Queue<I> {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| Vec::new()),
        }
    }
}
impl<I: Copy + Eq> Queue<I> {
    pub fn preflight(&self, entry: i32, owner: Target<I>, count: usize) -> Result<(), String> {
        let slot = usize::try_from(entry)
            .ok()
            .and_then(|e| self.slots.get(e))
            .ok_or("invalid grab command fighter entry")?;
        let used = if slot.first().is_some_and(|p| p.owner == owner) {
            slot.len()
        } else {
            0
        };
        if count > 64usize.saturating_sub(used) {
            return Err("grab command queue limit exceeded".into());
        }
        Ok(())
    }
    pub fn enqueue(&mut self, entry: i32, pending: Pending<I>) -> Result<(), String> {
        self.preflight(entry, pending.owner, 1)?;
        if !matches!(pending.command, Command::HeldDamage(v) if v.is_finite() && (0.0..=999.0).contains(&v))
            && !matches!(
                pending.command,
                Command::ThrowHeldOpponent(_) | Command::ReleaseHeldOpponent
            )
        {
            return Err("unsupported or unbounded grab command".into());
        }
        let slot = &mut self.slots[entry as usize];
        if slot.first().is_some_and(|p| p.owner != pending.owner) {
            slot.clear();
        }
        slot.push(pending);
        Ok(())
    }
    pub fn take(&mut self, boid: u32) -> Vec<Pending<I>> {
        let mut commands = Vec::new();
        for slot in &mut self.slots {
            let mut keep = Vec::new();
            for pending in std::mem::take(slot) {
                if pending.owner.boid == boid {
                    commands.push(pending);
                } else {
                    keep.push(pending);
                }
            }
            *slot = keep;
        }
        commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pending() -> Pending<u64> {
        Pending {
            owner: Target {
                boid: 1,
                identity: 11,
            },
            target: Target {
                boid: 2,
                identity: 22,
            },
            lease: Lease {
                status: 5,
                revision: 6,
            },
            command: Command::HeldDamage(4.0),
        }
    }
    #[test]
    fn queued_grab_actions_reject_lost_replaced_recycled_targets_and_expired_owners() {
        let pending = pending();
        pending
            .check(pending.owner, Some(pending.target), pending.lease)
            .unwrap();
        for target in [
            None,
            Some(Target {
                boid: 3,
                identity: 33,
            }),
            Some(Target {
                boid: 2,
                identity: 23,
            }),
        ] {
            assert!(pending
                .check(pending.owner, target, pending.lease)
                .unwrap_err()
                .contains("capture link changed"));
        }
        assert!(pending
            .check(
                Target {
                    boid: 1,
                    identity: 12
                },
                Some(pending.target),
                pending.lease
            )
            .is_err());
        for lease in [
            Lease {
                status: 7,
                revision: 6,
            },
            Lease {
                status: 5,
                revision: 7,
            },
        ] {
            assert!(pending
                .check(pending.owner, Some(pending.target), lease)
                .is_err());
        }
    }
    #[test]
    fn grab_queue_is_bounded_and_drains_only_the_owning_fighter() {
        let mut queue = Queue::default();
        for entry in [-1, 8] {
            assert!(queue.enqueue(entry, pending()).is_err());
        }
        for amount in [-1.0, 1000.0, f32::NAN, f32::INFINITY] {
            let mut bad = pending();
            bad.command = Command::HeldDamage(amount);
            assert!(queue.enqueue(0, bad).is_err());
        }
        let mut release = pending();
        release.command = Command::ReleaseHeldOpponent;
        queue.enqueue(1, release).unwrap();
        let mut other = pending();
        other.owner.boid = 3;
        queue.enqueue(2, other).unwrap();
        for _ in 0..64 {
            queue.enqueue(0, pending()).unwrap();
        }
        assert!(queue.preflight(0, pending().owner, 1).is_err());
        assert!(queue.enqueue(0, pending()).is_err());
        assert_eq!(queue.take(1).len(), 65);
        assert!(queue.take(1).is_empty());
        assert_eq!(queue.take(3).len(), 1);
        queue.preflight(0, pending().owner, 64).unwrap();
    }
    #[test]
    fn typed_throws_queue_without_following_a_replacement_target() {
        use super::super::native_inputs::ThrowDirection;
        for direction in [
            ThrowDirection::Forward,
            ThrowDirection::Back,
            ThrowDirection::Up,
            ThrowDirection::Down,
        ] {
            let mut action = pending();
            action.command = Command::ThrowHeldOpponent(direction);
            let mut queue = Queue::default();
            queue.enqueue(0, action).unwrap();
            let delivered = queue.take(action.owner.boid);
            assert_eq!(delivered[0].command, action.command);
            assert!(delivered[0]
                .check(action.owner, None, action.lease)
                .is_err());
            assert!(delivered[0]
                .check(
                    action.owner,
                    Some(Target {
                        boid: 99,
                        identity: 22
                    }),
                    action.lease
                )
                .is_err());
        }
    }
    fn sample(held: Option<Target<u64>>, owner_damage: f32, target_alive: bool) -> HoldSample<u64> {
        HoldSample {
            owned: true,
            held,
            owner_damage,
            target_alive,
            engaged: true,
        }
    }
    #[test]
    fn owned_hold_releases_on_hit_lost_target_and_timeout_and_keeps_otherwise() {
        let target = pending().target;
        let (row, reason) = step_owned_hold(None, sample(Some(target), 10.0, true));
        assert_eq!(reason, None);
        let row = row.unwrap();
        // The paired positive: an undisturbed hold survives and counts.
        let (kept, reason) = step_owned_hold(Some(row), sample(Some(target), 10.0, true));
        assert_eq!((kept.unwrap().frames, reason), (1, None));
        assert_eq!(
            step_owned_hold(Some(row), sample(Some(target), 12.0, true)).1,
            Some(ReleaseReason::OwnerHit)
        );
        assert_eq!(
            step_owned_hold(Some(row), sample(Some(target), 10.0, false)).1,
            Some(ReleaseReason::TargetLost)
        );
        assert_eq!(
            step_owned_hold(Some(row), sample(None, 10.0, true)).1,
            Some(ReleaseReason::TargetLost)
        );
        assert_eq!(
            step_owned_hold::<u64>(None, sample(None, 10.0, true)).1,
            None
        );
        assert_eq!(
            step_owned_hold(None, sample(Some(target), 10.0, false)).1,
            Some(ReleaseReason::TargetLost)
        );
        // A heal lowers the baseline; the next hit is still seen.
        let (healed, reason) = step_owned_hold(Some(row), sample(Some(target), 4.0, true));
        assert_eq!(reason, None);
        assert_eq!(
            step_owned_hold(healed, sample(Some(target), 5.0, true)).1,
            Some(ReleaseReason::OwnerHit)
        );
        let last = OwnedHold {
            frames: MAX_OWNED_HOLD_FRAMES - 2,
            ..row
        };
        assert_eq!(
            step_owned_hold(Some(last), sample(Some(target), 10.0, true)).1,
            None
        );
        let last = OwnedHold {
            frames: MAX_OWNED_HOLD_FRAMES - 1,
            ..row
        };
        assert_eq!(
            step_owned_hold(Some(last), sample(Some(target), 10.0, true)).1,
            Some(ReleaseReason::Timeout)
        );
        // A different target starts a new hold instead of inheriting the old count.
        let other = Target {
            boid: 9,
            identity: 99,
        };
        let (fresh, reason) = step_owned_hold(Some(last), sample(Some(other), 10.0, true));
        assert_eq!((fresh.unwrap().frames, reason), (0, None));
    }
    #[test]
    fn a_throw_that_never_finishes_is_rescued_again_until_both_are_free() {
        let target = pending().target;
        let outside = HoldSample {
            owned: false,
            held: None,
            owner_damage: 0.0,
            target_alive: true,
            engaged: true,
        };
        // A vanilla grab that never entered an owned status has no row and is never touched.
        for _ in 0..2 * MAX_UNOWNED_HOLD_FRAMES {
            assert!(matches!(
                step_owned_hold::<u64>(None, outside),
                (None, None)
            ));
        }
        let (mut row, _) = step_owned_hold(None, sample(Some(target), 0.0, true));
        let mut rescues = 0;
        for _ in 0..2 * MAX_UNOWNED_HOLD_FRAMES + 1 {
            let (next, reason) = step_owned_hold(row, outside);
            row = next;
            rescues += u32::from(reason == Some(ReleaseReason::Timeout));
            assert!(row.is_some());
        }
        assert_eq!(rescues, 2);
        // Both free ends it, with no rescue, including while a rescue was counting.
        assert!(matches!(
            step_owned_hold(
                row,
                HoldSample {
                    engaged: false,
                    ..outside
                }
            ),
            (None, None)
        ));
        // A finishing throw stays under the budget.
        let (mut row, _) = step_owned_hold(None, sample(Some(target), 0.0, true));
        for _ in 0..MAX_UNOWNED_HOLD_FRAMES {
            let (next, reason) = step_owned_hold(row, outside);
            assert_eq!(reason, None);
            row = next;
        }
        // Coming back into an owned status starts a fresh owned count.
        let (back, reason) = step_owned_hold(row, sample(Some(target), 0.0, true));
        assert_eq!(
            (back.unwrap().frames, back.unwrap().owned, reason),
            (0, true, None)
        );
    }
    #[test]
    fn exit_keeps_a_capture_only_for_a_status_that_manages_it() {
        let family = [0x38, 0x3C, 0x40];
        let target = pending().target;
        let exit = |next, next_owned| {
            exit_owned_status(None, Some(target), 0.0, next, next_owned, &family).1
        };
        assert!(exit(0x40, false));
        assert!(exit(0x1000, true));
        assert!(!exit(0x0, false));
    }
    #[test]
    fn a_capture_first_seen_at_owned_exit_is_rescued_in_a_vanilla_wait() {
        let target = pending().target;
        let outside = HoldSample {
            owned: false,
            held: Some(target),
            owner_damage: 0.0,
            target_alive: true,
            engaged: true,
        };
        // Nothing was observed held during the owned status, so there is no row before exit.
        let (mut row, keep) = exit_owned_status(None, Some(target), 0.0, 0x38, false, &[0x38]);
        assert!(keep);
        let mut frames = 0;
        let reason = loop {
            let (next, reason) = step_owned_hold(row, outside);
            row = next;
            frames += 1;
            if reason.is_some() || frames > MAX_UNOWNED_HOLD_FRAMES + 1 {
                break reason;
            }
        };
        assert_eq!(reason, Some(ReleaseReason::Timeout));
        assert_eq!(frames, MAX_UNOWNED_HOLD_FRAMES + 1);
        // No capture at exit means nothing to follow, and an existing count is not reset.
        assert!(
            exit_owned_status::<u64>(None, None, 0.0, 0x38, false, &[0x38])
                .0
                .is_none()
        );
        let counted = OwnedHold {
            target,
            owner_damage: 0.0,
            frames: 7,
            owned: true,
        };
        assert_eq!(
            exit_owned_status(Some(counted), Some(target), 9.0, 0x38, false, &[0x38])
                .0
                .unwrap()
                .frames,
            7
        );
    }
    #[test]
    fn release_then_damage_cannot_follow_a_new_capture() {
        let pending = pending();
        let mut target = Some(pending.target);
        let mut applied = 0;
        for command in [Command::ReleaseHeldOpponent, Command::HeldDamage(5.0)] {
            if pending.check(pending.owner, target, pending.lease).is_ok() {
                applied += 1;
                if command == Command::ReleaseHeldOpponent {
                    target = None;
                }
            }
        }
        assert_eq!(applied, 1);
    }
}

#[cfg(test)]
mod recycled_entry_tests {
    use super::*;
    #[test]
    fn recycled_entry_discards_old_actor_commands_without_retargeting() {
        let mut queue = Queue::default();
        let old = Pending {
            owner: Target {
                boid: 1,
                identity: 11,
            },
            target: Target {
                boid: 2,
                identity: 22,
            },
            lease: Lease {
                status: 5,
                revision: 6,
            },
            command: Command::HeldDamage(4.0),
        };
        for _ in 0..64 {
            queue.enqueue(0, old).unwrap();
        }
        let new = Pending {
            owner: Target {
                boid: 3,
                identity: 33,
            },
            ..old
        };
        queue.preflight(0, new.owner, 1).unwrap();
        queue.enqueue(0, new).unwrap();
        assert!(queue.take(1).is_empty());
        let commands = queue.take(3);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].owner, new.owner);
    }
}
