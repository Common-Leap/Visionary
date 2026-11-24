//! Who looks after the hidden item that carries edited effects into a match, and what is left of
//! one whose match has ended.
//!
//! Plain std only: the editor's tests include this file by path and run it, since tests in the
//! plugin crate never run.

/// The fighter whose frame it is, as the carrier sees it.
#[derive(Clone, Copy, Debug)]
pub struct Turn {
    /// The carrier has this fighter's edits in it.
    pub target: bool,
    /// This fighter holds the carrier.
    pub holds: bool,
    /// Another fighter that is still in the match holds it.
    pub held_elsewhere: bool,
    /// The carrier has that other fighter's edits in it.
    pub holder_is_target: bool,
}

/// Whether this fighter looks after the carrier in `state` (see `AUTO_CARRIER_STATE`). Only a
/// fighter it was built for is ever given one. Any fighter may let an old one go: its own may
/// never play again, and the next one waits for that.
pub fn looks_after(state: u64, turn: Turn) -> bool {
    match state {
        1 => turn.target && !(turn.held_elsewhere && turn.holder_is_target),
        2 => turn.holds || (turn.target && !turn.held_elsewhere),
        _ => !turn.held_elsewhere,
    }
}

/// What a swap finds of the carrier it replaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outgoing {
    /// None is left to remove.
    Removed,
    /// Its match ended and took the object with it: nothing will ever report it removed.
    GoneWithItsMatch,
    /// Up, but the effect file it loaded isn't known yet, so its release couldn't be seen.
    Unidentified,
    Up,
}

/// `object` is the carrier on record (0 = none), `alive` whether the game still has it, and
/// `handle` the effect file it loaded (0 = unknown, or already released).
pub fn outgoing(object: u64, alive: bool, handle: u64) -> Outgoing {
    match (object, alive, handle) {
        (0, _, _) => Outgoing::Removed,
        (_, false, _) => Outgoing::GoneWithItsMatch,
        (_, true, 0) => Outgoing::Unidentified,
        _ => Outgoing::Up,
    }
}

/// How many of an old carrier's effect names its file still answers to. `owners` is, for each
/// name, the effect file the game finds it in; `file` the carrier's (0 = unknown). A name some
/// fighter's own file answers to is that fighter's, and never goes away.
pub fn still_registered(owners: impl Iterator<Item = Option<u32>>, file: u64) -> usize {
    owners
        .flatten()
        .filter(|owner| file == 0 || u64::from(*owner) == file)
        .count()
}
