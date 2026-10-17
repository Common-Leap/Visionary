//! Frame data for one move: startup, active frames, first actionable frame, landing lag and
//! shield advantage — the numbers a moveset author tunes against.
//!
//! Every number is in **game frames**, the ones a player experiences, so `FT_MOTION_RATE` windows
//! are applied (see [`crate::data::AcmdScript::game_frame`]). Each value carries where it came
//! from, because a frame-data figure is only as useful as the trust it earns: "FAF 34 (motion
//! list cancel frame)" can be checked, a bare "34" cannot.
//!
//! The shieldstun formula is Ultimate's `floor(damage × 0.8 × mul + 2)`, where `mul` is 1 for
//! most moves, `shield_stiff_mul_attack_4` for smash attacks and `shield_stiff_mul_attack_air`
//! for aerials (both from `fighter/common/param/common.prc`). It uses the hitbox's base damage,
//! as published frame data does.

use std::path::Path;

/// Where a value came from, for the panel's tooltips.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// The motion list's `cancel_frame` for this motion.
    CancelFrame,
    /// No cancel frame: the move is actionable when its animation ends.
    AnimationEnd,
    /// A param in the game's (or a mod's) `.prc` files.
    Param(String),
    /// A param override written in the linked moveset's source (`param_config` and similar).
    SourceOverride(String),
    /// The game's default, used because the param file was not found.
    Default,
}

impl Source {
    pub fn describe(&self) -> String {
        match self {
            Source::CancelFrame => "the motion list's cancel_frame for this motion".into(),
            Source::AnimationEnd => "the end of the animation (the motion list sets no cancel_frame)".into(),
            Source::Param(name) => format!("the {name} param"),
            Source::SourceOverride(name) => format!("{name}, overridden in your linked source"),
            Source::Default => "the game's default value (param file not found)".into(),
        }
    }
}

/// What kind of move the motion name says this is, for landing lag and shieldstun.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MoveKind {
    Aerial(&'static str),
    Smash,
    Other,
}

impl MoveKind {
    pub fn of(motion: &str) -> Self {
        let motion = motion.to_ascii_lowercase();
        if let Some(rest) = motion.strip_prefix("attack_air_") {
            for (prefix, key) in [("hi", "hi"), ("lw", "lw"), ("n", "n"), ("f", "f"), ("b", "b")] {
                if rest.starts_with(prefix) {
                    return MoveKind::Aerial(key);
                }
            }
        }
        if ["attack_s4", "attack_hi4", "attack_lw4"].iter().any(|p| motion.starts_with(p)) {
            return MoveKind::Smash;
        }
        MoveKind::Other
    }

    pub fn landing_param(&self) -> Option<String> {
        match self {
            MoveKind::Aerial(key) => Some(format!("landing_attack_air_frame_{key}")),
            _ => None,
        }
    }
}

/// The params frame data needs, with where each came from.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameParams {
    pub landing_lag: Option<(f32, Source)>,
    pub shield_mul_smash: (f32, Source),
    pub shield_mul_air: (f32, Source),
}

impl Default for FrameParams {
    fn default() -> Self {
        FrameParams {
            landing_lag: None,
            shield_mul_smash: (0.725, Source::Default),
            shield_mul_air: (0.33, Source::Default),
        }
    }
}

/// One move's numbers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FrameData {
    /// First game frame a hitbox is out.
    pub startup: Option<u32>,
    /// Game-frame windows hitboxes are out, merged, inclusive.
    pub active: Vec<(u32, u32)>,
    /// First actionable frame, and where it came from.
    pub faf: Option<(u32, Source)>,
    pub landing_lag: Option<(u32, Source)>,
    /// Shieldstun of the strongest hitbox out on the startup frame.
    pub shieldstun: Option<u32>,
    /// `shieldstun - (FAF - startup)` for a grounded move; for an aerial, the advantage when
    /// the aerial lands right after hitting (`shieldstun - landing lag`).
    pub shield_advantage: Option<i32>,
    pub kind: Option<MoveKind>,
}

/// Inputs from the editor's current move.
pub struct Inputs<'a> {
    pub motion: &'a str,
    pub hitboxes: &'a [crate::data::Hitbox],
    pub script: &'a crate::data::AcmdScript,
    /// Animation length in motion frames, `0` when unknown.
    pub animation_frames: u32,
    /// The motion list's `cancel_frame`, `0`/`None` when unset.
    pub cancel_frame: Option<u32>,
    pub params: &'a FrameParams,
}

/// "Never cleared" in the parser's hitbox model.
const OPEN_ENDED: u32 = 9999;

pub fn compute(inputs: &Inputs) -> FrameData {
    let kind = MoveKind::of(inputs.motion);
    let game = |motion: u32| -> u32 { inputs.script.game_frame(motion as f32).ceil().max(1.0) as u32 };
    // Damaging collisions only: grabs, wind, ATTACK_ABS and detection boxes are not "active".
    let attacks: Vec<&crate::data::Hitbox> = inputs
        .hitboxes
        .iter()
        .filter(|hb| hb.category == 0 && hb.damage > 0.0)
        .collect();
    let mut windows: Vec<(u32, u32)> = attacks
        .iter()
        .map(|hb| {
            let end = if hb.active_end >= OPEN_ENDED {
                if inputs.animation_frames > 0 {
                    inputs.animation_frames.max(hb.active_start)
                } else {
                    hb.active_start
                }
            } else {
                hb.active_end
            };
            (game(hb.active_start), game(end))
        })
        .collect();
    windows.sort_unstable();
    let mut active: Vec<(u32, u32)> = Vec::new();
    for (start, end) in windows {
        match active.last_mut() {
            Some(last) if start <= last.1 + 1 => last.1 = last.1.max(end),
            _ => active.push((start, end)),
        }
    }
    let startup = active.first().map(|(start, _)| *start);

    let faf = match inputs.cancel_frame.filter(|frame| *frame > 0) {
        Some(cancel) => Some((game(cancel), Source::CancelFrame)),
        None if inputs.animation_frames > 0 => {
            Some((game(inputs.animation_frames) + 1, Source::AnimationEnd))
        }
        None => None,
    };

    let landing_lag = match kind {
        MoveKind::Aerial(_) => inputs
            .params
            .landing_lag
            .as_ref()
            .map(|(frames, source)| (frames.round().max(0.0) as u32, source.clone())),
        _ => None,
    };

    // The strongest hitbox out on the startup frame is the one a shield usually meets first.
    let shieldstun = startup.and_then(|first| {
        let damage = attacks
            .iter()
            .filter(|hb| game(hb.active_start) == first)
            .map(|hb| hb.damage)
            .fold(None, |best: Option<f32>, d| Some(best.map_or(d, |b| b.max(d))))?;
        let mul = match kind {
            MoveKind::Smash => inputs.params.shield_mul_smash.0,
            MoveKind::Aerial(_) => inputs.params.shield_mul_air.0,
            MoveKind::Other => 1.0,
        };
        Some(shieldstun(damage, mul))
    });
    let shield_advantage = match (kind, shieldstun) {
        (MoveKind::Aerial(_), Some(stun)) => landing_lag.as_ref().map(|(lag, _)| stun as i32 - *lag as i32),
        (_, Some(stun)) => match (&faf, startup) {
            (Some((faf, _)), Some(first)) => Some(stun as i32 - (*faf as i32 - first as i32)),
            _ => None,
        },
        _ => None,
    };

    FrameData {
        startup,
        active,
        faf,
        landing_lag,
        shieldstun,
        shield_advantage,
        kind: Some(kind),
    }
}

/// Ultimate's shieldstun for a hit of `damage` with move-type multiplier `mul`.
pub fn shieldstun(damage: f32, mul: f32) -> u32 {
    (damage * 0.8 * mul + 2.0).floor().max(0.0) as u32
}

/// Read the params for `fighter` (`pacman`) from the game data under `fighter_roots` — each a
/// folder holding `common/` and the fighters' folders, in load order, so a later one (a mod)
/// wins — letting `overrides` from the linked source win over all of them.
pub fn load_params(
    fighter_roots: &[std::path::PathBuf],
    fighter: &str,
    kind: MoveKind,
    overrides: &std::collections::HashMap<String, f64>,
) -> FrameParams {
    let mut params = FrameParams::default();
    let winning = |file: &str| -> Option<prc::ParamStruct> {
        fighter_roots
            .iter()
            .rev()
            .find_map(|root| read_prc(&root.join("common").join("param").join(file)))
    };
    if let Some(root) = winning("common.prc") {
        for (key, slot) in [
            ("shield_stiff_mul_attack_4", &mut params.shield_mul_smash),
            ("shield_stiff_mul_attack_air", &mut params.shield_mul_air),
        ] {
            if let Some(value) = find_number(&root, key) {
                *slot = (value, Source::Param(key.to_string()));
            }
        }
    }
    if let Some(key) = kind.landing_param() {
        if let Some(value) = overrides.get(&key) {
            params.landing_lag = Some((*value as f32, Source::SourceOverride(key)));
        } else if let Some(root) = winning("fighter_param.prc") {
            if let Some(row) = fighter_row(&root, fighter) {
                if let Some(value) = field_number(row, &key) {
                    params.landing_lag = Some((value, Source::Param(key)));
                }
            }
        }
    }
    params
}

/// `fighter`'s `scale` in `fighter_param.prc`, from the last of `fighter_roots` that has the file
/// (see [`load_params`]).
pub fn fighter_param_scale(fighter_roots: &[std::path::PathBuf], fighter: &str) -> Option<f32> {
    let root = fighter_roots.iter().rev().find_map(|root| {
        read_prc(&root.join("common").join("param").join("fighter_param.prc"))
    })?;
    field_number(fighter_row(&root, fighter)?, "scale").filter(|scale| *scale > 0.0)
}

fn read_prc(path: &Path) -> Option<prc::ParamStruct> {
    prc::open(path).ok()
}

fn hash(key: &str) -> u64 {
    hash40::hash40(key).0
}

fn number(value: &prc::ParamKind) -> Option<f32> {
    match value {
        prc::ParamKind::I8(v) => Some(*v as f32),
        prc::ParamKind::U8(v) => Some(*v as f32),
        prc::ParamKind::I16(v) => Some(*v as f32),
        prc::ParamKind::U16(v) => Some(*v as f32),
        prc::ParamKind::I32(v) => Some(*v as f32),
        prc::ParamKind::U32(v) => Some(*v as f32),
        prc::ParamKind::Float(v) => Some(*v),
        _ => None,
    }
}

fn field_number(param: &prc::ParamStruct, key: &str) -> Option<f32> {
    let key = hash(key);
    param
        .0
        .iter()
        .find_map(|(name, value)| (name.0 == key).then(|| number(value)).flatten())
}

/// A number anywhere in `root`, searched breadth-first: `common.prc` nests some params.
fn find_number(root: &prc::ParamStruct, key: &str) -> Option<f32> {
    if let Some(value) = field_number(root, key) {
        return Some(value);
    }
    root.0.iter().find_map(|(_, value)| match value {
        prc::ParamKind::Struct(inner) => find_number(inner, key),
        _ => None,
    })
}

fn fighter_row<'a>(root: &'a prc::ParamStruct, fighter: &str) -> Option<&'a prc::ParamStruct> {
    let table = hash("fighter_param_table");
    let kind_key = hash("fighter_kind");
    let wanted = hash(&format!("fighter_kind_{fighter}"));
    let list = root.0.iter().find_map(|(name, value)| match value {
        prc::ParamKind::List(list) if name.0 == table => Some(list),
        _ => None,
    })?;
    list.0.iter().find_map(|entry| match entry {
        prc::ParamKind::Struct(row)
            if row.0.iter().any(|(name, value)| {
                name.0 == kind_key && matches!(value, prc::ParamKind::Hash(h) if h.0 == wanted)
            }) =>
        {
            Some(row)
        }
        _ => None,
    })
}

/// `landing_attack_air_frame_f: 8.0` and its siblings, as a moveset's own source overrides
/// them (Shy Guy and Sceptile set landing lag through `param_config` this way). Commented-out
/// lines do not count.
pub fn source_param_overrides<'a>(
    sources: impl IntoIterator<Item = &'a str>,
) -> std::collections::HashMap<String, f64> {
    const KEYS: [&str; 5] = [
        "landing_attack_air_frame_n",
        "landing_attack_air_frame_f",
        "landing_attack_air_frame_b",
        "landing_attack_air_frame_hi",
        "landing_attack_air_frame_lw",
    ];
    let mut found: std::collections::HashMap<String, Option<f64>> = std::collections::HashMap::new();
    for text in sources {
        for line in text.lines() {
            let code = line.split("//").next().unwrap_or("").trim();
            for key in KEYS {
                let Some(rest) = code.strip_prefix(key) else { continue };
                let Some(value) = rest.trim_start().strip_prefix(':') else { continue };
                let value = value.trim().trim_end_matches(',').trim();
                let Ok(number) = value.trim_end_matches("f32").parse::<f64>() else { continue };
                found
                    .entry(key.to_string())
                    .and_modify(|known| {
                        if *known != Some(number) {
                            *known = None; // two different overrides: no single answer
                        }
                    })
                    .or_insert(Some(number));
            }
        }
    }
    found
        .into_iter()
        .filter_map(|(key, value)| Some((key, value?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hitbox(start: u32, end: u32, damage: f32) -> crate::data::Hitbox {
        crate::data::Hitbox {
            active_start: start,
            active_end: end,
            damage,
            ..Default::default()
        }
    }

    #[test]
    fn shieldstun_matches_published_values() {
        assert_eq!(shieldstun(8.0, 0.33), 4, "Mario nair");
        assert_eq!(shieldstun(2.2, 1.0), 3, "a light jab");
        assert_eq!(shieldstun(17.8, 0.725), 12, "Mario fsmash");
    }

    #[test]
    fn a_grounded_move_reads_startup_active_faf_and_advantage() {
        let script = crate::data::AcmdScript::default();
        let hitboxes = [hitbox(5, 7, 10.0), hitbox(5, 6, 8.0), hitbox(9, 10, 4.0)];
        let params = FrameParams::default();
        let data = compute(&Inputs {
            motion: "attack_s3_s",
            hitboxes: &hitboxes,
            script: &script,
            animation_frames: 30,
            cancel_frame: Some(26),
            params: &params,
        });
        assert_eq!(data.startup, Some(5));
        assert_eq!(data.active, [(5, 7), (9, 10)]);
        assert_eq!(data.faf, Some((26, Source::CancelFrame)));
        assert_eq!(data.shieldstun, Some(10), "floor(10 × 0.8 + 2)");
        assert_eq!(data.shield_advantage, Some(10 - (26 - 5)));
    }

    #[test]
    fn a_motion_rate_moves_every_number_onto_game_frames() {
        // Half rate from frame 1: each motion frame takes half a game frame.
        let script = crate::acmd::parse_acmd_script(
            "unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {\n    macros::FT_MOTION_RATE(agent, 0.5);\n    frame(agent.lua_state_agent, 10.0);\n}\n",
        );
        let hitboxes = [hitbox(10, 12, 5.0)];
        let data = compute(&Inputs {
            motion: "attack_11",
            hitboxes: &hitboxes,
            script: &script,
            animation_frames: 0,
            cancel_frame: Some(20),
            params: &FrameParams::default(),
        });
        assert_eq!(data.startup, Some(5));
        assert_eq!(data.faf.map(|(f, _)| f), Some(10));
    }

    /// Startup and FAF for a 10-frame hitbox and a cancel at 20 under `body`.
    fn startup_and_faf(body: &str) -> (Option<u32>, Option<u32>) {
        let script = crate::acmd::parse_acmd_script(&format!(
            "unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {{\n{body}\n    frame(agent.lua_state_agent, 10.0);\n}}\n"
        ));
        let hitboxes = [hitbox(10, 12, 5.0)];
        let data = compute(&Inputs {
            motion: "attack_11",
            hitboxes: &hitboxes,
            script: &script,
            animation_frames: 0,
            cancel_frame: Some(20),
            params: &FrameParams::default(),
        });
        (data.startup, data.faf.map(|(f, _)| f))
    }

    #[test]
    fn every_rate_form_moves_frame_data() {
        // smash_script's helpers: both call FT_MOTION_RATE with game_frames / motion_frames.
        assert_eq!(startup_and_faf("    FT_MOTION_RATE_RANGE(agent, 0.0, 10.0, 5.0);"), (Some(5), Some(10)));
        assert_eq!(startup_and_faf("    macros::FT_DESIRED_RATE(agent, 10.0, 20.0);"), (Some(20), Some(40)));
        // set_rate is playback speed: 2.0 plays twice as fast, so half the game frames.
        assert_eq!(
            startup_and_faf("    if macros::is_excute(agent) {\n        MotionModule::set_rate(agent.module_accessor, 2.0);\n    }"),
            (Some(5), Some(10))
        );
    }

    #[test]
    fn waits_inside_a_loop_still_move_a_later_rate_change() {
        // Four waits of 2 put the cursor at 8; the rate set there halves only what follows.
        let (startup, faf) = startup_and_faf(
            "    for _ in 0..4 {\n        wait(agent.lua_state_agent, 2.0);\n    }\n    macros::FT_MOTION_RATE(agent, 0.5);",
        );
        assert_eq!(startup, Some(9), "8 + 2 × 0.5");
        assert_eq!(faf, Some(14), "8 + 12 × 0.5");
    }

    #[test]
    fn an_aerial_uses_landing_lag_and_the_air_multiplier() {
        let script = crate::data::AcmdScript::default();
        let hitboxes = [hitbox(3, 20, 8.0)];
        let params = FrameParams {
            landing_lag: Some((8.0, Source::SourceOverride("landing_attack_air_frame_n".into()))),
            ..FrameParams::default()
        };
        let data = compute(&Inputs {
            motion: "attack_air_n",
            hitboxes: &hitboxes,
            script: &script,
            animation_frames: 45,
            cancel_frame: None,
            params: &params,
        });
        assert_eq!(data.kind, Some(MoveKind::Aerial("n")));
        assert_eq!(data.faf, Some((46, Source::AnimationEnd)));
        assert_eq!(data.shieldstun, Some(4));
        assert_eq!(data.shield_advantage, Some(4 - 8));
    }

    #[test]
    fn source_overrides_ignore_comments_and_disagreements() {
        let params = r#"
            // landing_attack_air_frame_n: 7.0,
            landing_attack_air_frame_f: 8.0,
            landing_attack_air_frame_b: 12.0, // tuned
        "#;
        let other = "landing_attack_air_frame_b: 13.0,";
        let found = source_param_overrides([params, other]);
        assert_eq!(found.get("landing_attack_air_frame_f"), Some(&8.0));
        assert!(!found.contains_key("landing_attack_air_frame_n"));
        assert!(!found.contains_key("landing_attack_air_frame_b"));
    }
}
