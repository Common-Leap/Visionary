//! When an added or moved effect fires: once each time its motion plays, on the first frame at
//! or past its own. One rule for fighters and their objects, however the frame is reached (a
//! script's `frame`/`wait` or the once-per-frame tick) and whatever the motion's rate.
//!
//! A motion that carries on from another's frame (a grounded special leaving the ground on frame
//! 20) plays from that frame: what is behind it stays behind, as in the game's own scripts.
//! The same goes for a rule that arrives while its motion is already playing.
//!
//! Plain std only: the editor's tests include this file by path and run it, since tests in the
//! plugin crate never run.

use std::collections::HashMap;

/// Before the first frame, so that frame 0 is still ahead.
const START: f32 = -1.0;
/// A first dispatch can be made before the game can take it; it gets one more on the same frame.
pub const MAX_ATTEMPTS: u8 = 2;
/// How far past its frame a call still counts as on it.
const FRAME_WIDTH: f32 = 0.5;

struct Fired {
    slot: usize,
    identity: u64,
    attempts: u8,
    confirmed: bool,
}

/// One playing of a motion by one object.
pub struct Playing {
    motion: u64,
    /// The frame it was last seen on.
    frame: f32,
    /// Frames up to and including this one are behind: played, or passed over at the start.
    passed: f32,
    /// What the motion it carried on from had already reached.
    inherited: f32,
    /// Told to start, not yet seen on a frame.
    fresh: bool,
    /// Had its first once-per-frame tick.
    ticked: bool,
    fired: Vec<Fired>,
}

impl Playing {
    fn new(motion: u64, frame: f32, step: f32) -> Self {
        let first = frame <= step.max(1.0);
        Self {
            motion,
            frame,
            passed: if first { START } else { frame - step },
            inherited: START,
            fresh: false,
            ticked: false,
            fired: Vec::new(),
        }
    }

    /// The once-per-frame tick. Whether it is the playing's first, when nothing can fire yet: a
    /// motion's game and expression scripts start before it, and an effect made then, or on it,
    /// is there but never drawn. Its effect script starts after it.
    pub fn tick(&mut self) -> bool {
        !std::mem::replace(&mut self.ticked, true)
    }

    /// Whether what `slot` holds is due now that the motion has reached `frame`.
    pub fn due(&self, slot: usize, identity: u64, target: f32, frame: f32) -> bool {
        self.ticked
            && self.passed < target
            && target <= frame
            && self
                .fired(slot, identity)
                .is_none_or(|fired| !fired.confirmed && fired.attempts < MAX_ATTEMPTS)
    }

    /// Record one dispatch of `slot` and how many it has had.
    pub fn fire(&mut self, slot: usize, identity: u64, confirmed: bool) -> u8 {
        if let Some(fired) = self
            .fired
            .iter_mut()
            .find(|fired| fired.slot == slot && fired.identity == identity)
        {
            fired.attempts += 1;
            fired.confirmed |= confirmed;
            return fired.attempts;
        }
        self.fired.push(Fired {
            slot,
            identity,
            attempts: 1,
            confirmed,
        });
        1
    }

    /// Every frame up to this one has had its turn.
    pub fn pass(&mut self, frame: f32) {
        self.passed = self.passed.max(frame);
    }

    /// Whether a move of an effect to `target` came too late for this playing: its frame is
    /// behind and it never fired. One the motion it carried on from had reached is not late.
    pub fn missed(&self, slot: usize, identity: u64, target: f32, frame: f32) -> bool {
        self.inherited < target
            && target + FRAME_WIDTH <= frame
            && !self
                .fired(slot, identity)
                .is_some_and(|fired| fired.confirmed)
    }

    fn fired(&self, slot: usize, identity: u64) -> Option<&Fired> {
        self.fired
            .iter()
            .find(|fired| fired.slot == slot && fired.identity == identity)
    }
}

/// The current playing of every object that has one, by battle object id.
#[derive(Default)]
pub struct Playings(HashMap<u32, Playing>);

impl Playings {
    /// The game started `motion` on this object. `inherits`: it carries on from the frame the
    /// object's last motion was on.
    pub fn started(&mut self, object: u32, motion: u64, inherits: bool) {
        let Some(last) = self.0.get_mut(&object) else {
            return;
        };
        let reached = if inherits { last.frame } else { START };
        *last = Playing {
            motion,
            frame: reached,
            passed: reached,
            inherited: reached,
            fresh: !inherits,
            ticked: false,
            fired: Vec::new(),
        };
    }

    /// The object is on `frame` of `motion`, which moves `step` frames at a time.
    pub fn at(&mut self, object: u32, motion: u64, frame: f32, step: f32) -> &mut Playing {
        let playing = self
            .0
            .entry(object)
            .or_insert_with(|| Playing::new(motion, frame, step));
        if playing.motion != motion {
            // A change the hooks did not see. Frame 20 after frame 19 or 20 carries on.
            let reached = playing.frame;
            *playing = Playing::new(motion, frame, step);
            if playing.passed >= 0.0 && reached <= frame && reached > playing.passed {
                playing.passed = reached;
            }
            playing.inherited = playing.passed;
        } else if playing.fresh || frame < playing.frame {
            // A start, or a motion that went back: it looped or began again.
            *playing = Playing::new(motion, frame, step);
        }
        playing.frame = frame;
        playing
    }

    /// The playing `at` last answered for this object.
    pub fn current(&mut self, object: u32) -> Option<&mut Playing> {
        self.0.get_mut(&object)
    }

    /// Keep only the objects `alive` says are still there.
    pub fn retain(&mut self, alive: impl Fn(u32) -> bool) {
        self.0.retain(|object, _| alive(*object));
    }
}
