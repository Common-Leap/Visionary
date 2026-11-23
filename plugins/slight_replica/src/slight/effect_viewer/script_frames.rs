//! Which frame an object's script is on, counted the way the editor counts it: 0 where the script
//! starts (which it calls frame 1), N after `frame(N)`, n more after `wait(n)`.
//!
//! A fighter's script is on frame N while its motion is on frame N-1, so its calls are matched to
//! rules by the motion's frame. An object's is not: its motion is on 0 at the start, on 2 after a
//! `wait(2)`, but on 5 after a `frame(5)`. Its own count is kept here instead, so a rule names the
//! same call for both.
//!
//! Plain std only: the editor's tests include this file by path and run it, since tests in the
//! plugin crate never run.

use std::collections::HashMap;

struct Script {
    object: u32,
    motion: u64,
    /// The motion's frame when the script last ran.
    seen: f32,
    /// The script's own frame.
    frame: f32,
}

/// Every object script that is running, by the Lua state it runs in.
#[derive(Default)]
pub struct ScriptFrames(HashMap<u64, Script>);

impl ScriptFrames {
    /// The script in `state` is running, with its object's `motion` on `seen`.
    pub fn runs(&mut self, state: u64, object: u32, motion: u64, seen: f32) {
        self.at(state, object, motion, seen);
    }

    /// Its `frame(target)` has been reached.
    pub fn reached(&mut self, state: u64, object: u32, motion: u64, seen: f32, target: f32) {
        let script = self.at(state, object, motion, seen);
        script.frame = script.frame.max(target);
    }

    /// Its `wait(frames)` is over.
    pub fn waited(&mut self, state: u64, object: u32, motion: u64, seen: f32, frames: f32) {
        self.at(state, object, motion, seen).frame += frames;
    }

    /// The frame a rule names for a call the script in `state` makes now: one less than the
    /// script's, as for a fighter's.
    pub fn rule_frame(&self, state: u64, object: u32) -> Option<f32> {
        self.0
            .get(&state)
            .filter(|script| script.object == object)
            .map(|script| script.frame.max(1.0) - 1.0)
    }

    /// The object starts a motion, and with it its scripts.
    pub fn restarted(&mut self, object: u32) {
        self.0.retain(|_, script| script.object != object);
    }

    /// Keep only the objects `alive` says are still there.
    pub fn retain(&mut self, alive: impl Fn(u32) -> bool) {
        self.0.retain(|_, script| alive(script.object));
    }

    fn at(&mut self, state: u64, object: u32, motion: u64, seen: f32) -> &mut Script {
        let start = || Script {
            object,
            motion,
            seen,
            frame: 0.0,
        };
        let script = self.0.entry(state).or_insert_with(start);
        // Another object's, another motion's, or the same motion gone back: a new run.
        if script.object != object || script.motion != motion || seen < script.seen {
            *script = start();
        }
        script.seen = seen;
        script
    }
}

/// The frame an object's motion is on, in a script's count less one: what an added call's frame
/// is tested against, so it comes out with the script's own `frame` of that number.
pub fn object_frame(motion_frame: f32) -> f32 {
    (motion_frame - 1.0).max(0.0)
}
