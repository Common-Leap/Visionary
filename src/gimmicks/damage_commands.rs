//! Fighter-local damage commands and their bounded lifetime bookkeeping.
//!
//! Percent changes persist as ordinary damage. Combat policies are
//! last-write-wins per channel (never multiplicative stacking), each held for
//! its scope: the status, a frame count, or until cleared. Delivery/cleanup
//! belongs to the owning fighter's Main callback. Cleanup restores only
//! controller-touched channels to normal when their scope ends, on
//! configuration replacement, or teardown. No vanilla reset behavior is
//! assumed. A multiplier of 1.0 or a Clear releases that channel. Controllers
//! sharing a fighter share these native channels.
//! Item commands ride the same Main-line queue so items are never spawned
//! from inside a hit callback.
use super::native_inputs::{CombatChannel, CombatOp, PolicyScope};
use super::phase_commands::Command;

pub fn is_damage(command: Command) -> bool {
    !matches!(
        command,
        Command::HeldDamage(_)
            | Command::ReleaseHeldOpponent
            | Command::Rate(_)
            | Command::Speed { .. }
    )
}

pub fn damage_delta(current: f32, amount: f32) -> Result<f32, String> {
    if !current.is_finite() || current < 0.0 || !amount.is_finite() || amount.abs() > 999.0 {
        return Err("invalid damage percent or delta".into());
    }
    Ok(amount.max(-current))
}

/// Channels a controller changed, each with the scope that returns it to
/// normal and the op that set it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Touched {
    /// Indexed by `CombatChannel as usize`; `Frames` counts down once per delivery boundary.
    pub combat: [Option<(PolicyScope, CombatOp<f32>)>; CombatChannel::ALL.len()],
    /// Status-scoped, like a `PolicyScope::Status` channel.
    pub gravity: bool,
    /// Meshes a controller hid, shown again when the controller's hold on the
    /// fighter ends: stock loss or a configuration change. No status ends it, and
    /// the game has no per-mesh read, so a mesh vanilla already hid comes back too.
    /// Zero marks a free slot; past the last slot only a motion track restores it.
    pub hidden_meshes: [u64; 8],
}
impl Touched {
    pub fn record(&mut self, command: Command) {
        match command {
            Command::Combat(_, CombatOp::Clear(channel)) => self.combat[channel as usize] = None,
            Command::Combat(scope, op) => {
                let neutral = matches!(op, CombatOp::DamageTaken(v) | CombatOp::KnockbackTaken(v) | CombatOp::DamageDealt(v) | CombatOp::KnockbackDealt(v) | CombatOp::Stat(_, v) if v == 1.0);
                self.combat[op.channel() as usize] = (!neutral).then_some((scope, op));
            }
            Command::Movement(super::native_inputs::MovementAction::SetGravity { enabled }) => {
                self.gravity = !enabled
            }
            Command::MeshVisibility { mesh, visible } => {
                self.hidden_meshes
                    .iter_mut()
                    .filter(|slot| **slot == mesh)
                    .for_each(|slot| *slot = 0);
                if !visible {
                    if let Some(slot) = self.hidden_meshes.iter_mut().find(|slot| **slot == 0) {
                        *slot = mesh;
                    }
                }
            }
            _ => {}
        }
    }
    pub fn channel(channel: CombatChannel) -> Self {
        let mut touched = Self::default();
        touched.combat[channel as usize] =
            Some((PolicyScope::UntilCleared, CombatOp::Clear(channel)));
        touched
    }
    pub fn has(&self, channel: CombatChannel) -> bool {
        self.combat[channel as usize].is_some()
    }
    /// Advance one delivery boundary and take the channels whose scope ended.
    fn expire(&mut self, status_changed: bool, configuration_changed: bool) -> Touched {
        if configuration_changed {
            return std::mem::take(self);
        }
        let mut ended = Touched {
            gravity: status_changed && std::mem::take(&mut self.gravity),
            ..Touched::default()
        };
        for (slot, out) in self.combat.iter_mut().zip(ended.combat.iter_mut()) {
            let end = match slot {
                Some((PolicyScope::Status, _)) => status_changed,
                Some((PolicyScope::Frames(left), _)) => {
                    *left = left.saturating_sub(1);
                    *left == 0
                }
                _ => false,
            };
            if end {
                *out = slot.take();
            }
        }
        ended
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lease {
    pub status: i32,
    pub revision: u64,
}

pub struct Actor<I> {
    pub boid: u32,
    pub fighter: String,
    pub identity: I,
    pub lease: Lease,
    pub touched: Touched,
    pub pending: Vec<(Lease, Command)>,
}

impl<I> Actor<I> {
    /// Once per delivery boundary: return channels whose scope ended and
    /// discard queued commands from expired statuses/configurations before
    /// any native mutation.
    pub fn prepare(&mut self, current: Lease) -> (Touched, Vec<Command>) {
        let status_changed = self.lease.status != current.status;
        let reset = self
            .touched
            .expire(status_changed, self.lease.revision != current.revision);
        self.lease = current;
        // Vanilla status changes may reset armor and hit status; channels held
        // past the status are written again, with their remaining frames.
        // Stats live in the plugin's getter overlay, which no status touches.
        let held = self
            .touched
            .combat
            .iter()
            .flatten()
            .filter(|(_, op)| status_changed && !matches!(op, CombatOp::Stat(..)))
            .map(|(scope, op)| Command::Combat(*scope, *op));
        let commands = held
            .collect::<Vec<_>>()
            .into_iter()
            .chain(
                std::mem::take(&mut self.pending)
                    .into_iter()
                    .filter_map(|(lease, command)| (lease == current).then_some(command)),
            )
            .collect();
        (reset, commands)
    }
}

pub struct Queue<I> {
    slots: [Option<Actor<I>>; 8],
}
impl<I> Default for Queue<I> {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
        }
    }
}
impl<I: Copy + Eq> Queue<I> {
    pub fn identity(&self, boid: u32) -> Option<I> {
        self.slots
            .iter()
            .flatten()
            .find(|a| a.boid == boid)
            .map(|a| a.identity)
    }
    pub fn preflight(&self, boid: u32, identity: I, count: usize) -> Result<(), String> {
        let pending = self.slots.iter().flatten().find(|a| a.boid == boid);
        let used = pending
            .filter(|a| a.identity == identity)
            .map_or(0, |a| a.pending.len());
        if count > 64usize.saturating_sub(used) {
            return Err("damage command queue limit exceeded".into());
        }
        if pending.is_none() && self.slots.iter().all(Option::is_some) {
            return Err("damage command actor limit exceeded".into());
        }
        Ok(())
    }
    pub fn enqueue(
        &mut self,
        boid: u32,
        fighter: &str,
        identity: I,
        lease: Lease,
        command: Command,
    ) -> Result<(), String> {
        if !is_damage(command) {
            return Err("unsupported damage command".into());
        }
        let index = self
            .slots
            .iter()
            .position(|s| s.as_ref().is_some_and(|a| a.boid == boid))
            .or_else(|| self.slots.iter().position(Option::is_none))
            .ok_or("damage command actor limit exceeded")?;
        if self.slots[index]
            .as_ref()
            .is_none_or(|a| a.identity != identity || a.fighter != fighter)
        {
            self.slots[index] = Some(Actor {
                boid,
                fighter: fighter.into(),
                identity,
                lease,
                touched: Touched::default(),
                pending: Vec::new(),
            });
        }
        let actor = self.slots[index].as_mut().unwrap();
        if actor.pending.len() >= 64 {
            return Err("damage command queue limit exceeded".into());
        }
        actor.pending.push((lease, command));
        Ok(())
    }
    pub fn take(&mut self, boid: u32) -> Option<Actor<I>> {
        self.slots
            .iter_mut()
            .find(|s| s.as_ref().is_some_and(|a| a.boid == boid))?
            .take()
    }
    pub fn retain(&mut self, actor: Actor<I>) -> Result<(), String> {
        if actor.touched == Touched::default() {
            return Ok(());
        }
        if let Some(existing) = self
            .slots
            .iter_mut()
            .flatten()
            .find(|a| a.boid == actor.boid)
        {
            // Native calls may enqueue more work. Preserve that work while
            // putting the delivered channel ownership back into its slot.
            if existing.identity == actor.identity && existing.fighter == actor.fighter {
                existing.touched = actor.touched;
                existing.lease = actor.lease;
            }
            return Ok(());
        }
        let slot = self
            .slots
            .iter_mut()
            .find(|s| s.is_none())
            .ok_or("damage command actor limit exceeded during delivery")?;
        *slot = Some(actor);
        Ok(())
    }
}
