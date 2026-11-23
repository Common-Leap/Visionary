//! Viewport preview of AFTER_IMAGE sword trails.
//!
//! A trail is a ribbon stretched between two edges, each an offset from a named joint, swept
//! along as the animation moves those joints. The viewport used to mark one with the same
//! circle an ordinary spawn gets, at its first joint with no offset, which shows neither the
//! blade it follows nor the arc it draws. This module reads the edge geometry back out of the
//! call and works out which stretch of the swing is on screen at a given frame; `app.rs` samples
//! the skeleton along that stretch and paints the ribbon.
//!
//! The editor's own fields win over the source text: the joints come from `bone_name` and
//! `trail_bone2`, which the panel edits, and only the offsets — which no panel field exposes —
//! are read from the call verbatim.

use crate::data::EffectCall;

/// Argument slots of a trail call, counting `agent` (or the raw command id) as slot 0. The same
/// numbering as `acmd::TRAIL_*_SLOT`; see `data::RAW_TRAIL_COMMANDS` for why the wrapper and the
/// raw command share it.
const LENGTH_SLOT: usize = 3;
const EDGE1_OFFSET_SLOTS: [usize; 3] = [5, 6, 7];
const EDGE2_OFFSET_SLOTS: [usize; 3] = [9, 10, 11];

/// How many frames of swing the ribbon spans when the call's length slot cannot be read.
pub const DEFAULT_TRAIL_FRAMES: f32 = 8.0;
/// Ribbon samples per game frame. Fast swings cover a lot of arc in one frame, and a straight
/// quad per frame turns a curve into a fan of visible facets.
pub const SAMPLES_PER_FRAME: u32 = 3;

/// The two edges of a trail and how long its tail lasts.
#[derive(Clone, Debug, PartialEq)]
pub struct TrailGeometry {
    pub bone1: String,
    pub offset1: [f32; 3],
    pub bone2: String,
    pub offset2: [f32; 3],
    /// Tail length in game frames. Slot 3 of the call — 12 on Kirby's Final Cutter, 10 on Shy
    /// Guy's hammer — read as the number of frames of swing the trail keeps behind the blade.
    pub frames: f32,
}

/// The argument text of the trail call in `line`, slot-indexed as described above.
fn trail_args(line: &str) -> Option<Vec<String>> {
    let site = crate::acmd_src::raw_trail_line(line).or_else(|| {
        crate::acmd_src::scan_macro_sites_unscoped(line, 0..line.len())
            .into_iter()
            .find(|site| site.name.starts_with("AFTER_IMAGE"))
    })?;
    Some(
        (0..site.args.len())
            .map(|i| site.arg(line, i).unwrap_or_default().trim().to_string())
            .collect(),
    )
}

/// A numeric literal, or 0 for anything symbolic. Shy Guy writes `-z` for an edge offset; the
/// preview has no value for that and centring it on the joint is the least misleading guess.
fn number(token: Option<&String>) -> f32 {
    token
        .and_then(|t| {
            t.trim()
                .trim_end_matches("f32")
                .trim_end_matches('.')
                .parse::<f32>()
                .ok()
        })
        .filter(|v| v.is_finite())
        .unwrap_or(0.0)
}

/// The edge geometry of a trail call, or `None` when the call is not a trail or carries no
/// source text to read offsets from.
pub fn trail_geometry(call: &EffectCall) -> Option<TrailGeometry> {
    let line = call.raw_line.as_deref()?;
    let args = trail_args(line)?;
    if args.len() <= EDGE2_OFFSET_SLOTS[2] {
        return None;
    }
    let offset = |slots: [usize; 3]| slots.map(|slot| number(args.get(slot)));
    let frames = args
        .get(LENGTH_SLOT)
        .and_then(|t| t.parse::<f32>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(DEFAULT_TRAIL_FRAMES);
    Some(TrailGeometry {
        bone1: call.bone_name.clone(),
        offset1: offset(EDGE1_OFFSET_SLOTS),
        bone2: call
            .trail_bone2
            .clone()
            .unwrap_or_else(|| call.bone_name.clone()),
        offset2: offset(EDGE2_OFFSET_SLOTS),
        frames,
    })
}

/// The stretch of swing visible at game frame `current`, as `(tail, head)` in game frames.
///
/// The head follows the blade from the call's frame until the trail is switched off; after
/// that it stays where the blade was, and the tail keeps advancing until it catches up and the
/// ribbon is gone. `None` when nothing of the trail is on screen.
pub fn visible_span(
    active_start: u32,
    active_end: u32,
    frames: f32,
    current: f32,
) -> Option<(f32, f32)> {
    let start = active_start as f32;
    let head = current.min(active_end as f32);
    let tail = (current - frames).max(start);
    (current >= start && head > tail).then_some((tail, head))
}

/// Sample frames from `tail` to `head` inclusive, evenly spaced at [`SAMPLES_PER_FRAME`].
pub fn sample_frames(tail: f32, head: f32) -> Vec<f32> {
    let steps = (((head - tail) * SAMPLES_PER_FRAME as f32).ceil() as usize).max(1);
    (0..=steps)
        .map(|i| tail + (head - tail) * i as f32 / steps as f32)
        .collect()
}

/// The `.nuanmb` frame for a fractional game frame: one-based game frames, zero-based motion.
pub fn motion_frame(game_frame: f32) -> f32 {
    (game_frame - 1.0).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus_call() -> EffectCall {
        crate::acmd::parse_effect_script(&crate::acmd::tests::corpus_trail_script())
            .to_effect_calls()
            .into_iter()
            .find(|call| call.raw_line.is_some())
            .expect("trail call")
    }

    #[test]
    fn reads_both_edges_and_the_length_from_the_raw_corpus_trail() {
        let geometry = trail_geometry(&corpus_call()).expect("geometry");
        assert_eq!(geometry.bone1, "haver");
        assert_eq!(geometry.bone2, "haver");
        assert_eq!(geometry.offset1, [0.0, 3.0, 0.25]);
        assert_eq!(geometry.offset2, [0.0, 26.0, 0.5]);
        assert_eq!(geometry.frames, 12.0);
    }

    #[test]
    fn the_panels_joints_win_over_the_source_text() {
        let mut call = corpus_call();
        call.bone_name = "hip".into();
        call.trail_bone2 = Some("sword2".into());
        let geometry = trail_geometry(&call).expect("geometry");
        assert_eq!(
            (geometry.bone1.as_str(), geometry.bone2.as_str()),
            ("hip", "sword2")
        );
    }

    #[test]
    fn reads_the_wrapper_form_and_zeroes_a_symbolic_offset() {
        let mut call = corpus_call();
        call.raw_line = Some(
            r#"macros::AFTER_IMAGE4_ON_arg29(agent, Hash40::new("a"), Hash40::new("b"), 10, ball, 0, 3.0, -z, ball, 0, -3.0, z, true, Hash40::new("null"), ball, 0, 0, 0, 0, 0, 0, 1, 0, *EFFECT_AXIS_X, 0, *TRAIL_BLEND_ALPHA, 101, *TRAIL_CULL_NONE, 1.4, 0.1);"#
                .into(),
        );
        let geometry = trail_geometry(&call).expect("geometry");
        assert_eq!(geometry.offset1, [0.0, 3.0, 0.0]);
        assert_eq!(geometry.offset2, [0.0, -3.0, 0.0]);
        assert_eq!(geometry.frames, 10.0);
    }

    #[test]
    fn an_ordinary_spawn_has_no_trail_geometry() {
        let call = crate::acmd::parse_effect_script(&crate::acmd::tests::corpus_trail_script())
            .to_effect_calls()
            .into_iter()
            .find(|call| call.raw_line.is_none())
            .expect("follow call");
        assert_eq!(trail_geometry(&call), None);
    }

    #[test]
    fn the_ribbon_grows_holds_after_off_and_then_drains() {
        // On at 1, off at 7, 4 frames of tail.
        assert_eq!(visible_span(1, 7, 4.0, 0.5), None);
        assert_eq!(visible_span(1, 7, 4.0, 1.0), None);
        assert_eq!(visible_span(1, 7, 4.0, 3.0), Some((1.0, 3.0)));
        assert_eq!(visible_span(1, 7, 4.0, 6.0), Some((2.0, 6.0)));
        assert_eq!(visible_span(1, 7, 4.0, 9.0), Some((5.0, 7.0)));
        assert_eq!(visible_span(1, 7, 4.0, 11.0), None);
    }

    #[test]
    fn an_open_ended_trail_never_drains() {
        let open = crate::data::OPEN_ENDED_EFFECT_FRAME;
        assert_eq!(visible_span(1, open, 4.0, 50.0), Some((46.0, 50.0)));
    }

    #[test]
    fn samples_cover_the_span_end_to_end() {
        let samples = sample_frames(2.0, 4.0);
        assert_eq!(samples.first(), Some(&2.0));
        assert_eq!(samples.last(), Some(&4.0));
        assert_eq!(samples.len(), 2 * SAMPLES_PER_FRAME as usize + 1);
    }
}
