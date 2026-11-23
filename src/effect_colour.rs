//! The colour an effect is drawn in, for the 3D preview.
//!
//! The game multiplies an effect's own colours by its set colour. Two things set it:
//!
//! - the spawn itself: the `_COLOR` spawn forms carry red, green and blue as their last three
//!   arguments, the `_ALPHA` forms an opacity, and `LAST_EFFECT_SET_COLOR` /
//!   `LAST_EFFECT_SET_ALPHA` lines after a spawn recolour it;
//! - the fighter, for its attack arcs: `fighter_param.prc` gives every fighter a
//!   `common_effect_0` colour, and the game puts it on the `sys_attack_arc` effects (live probe:
//!   Dark Samus's up tilt arc draws with 0.32, 1.6, 0.4, her row of the table; the dust, lines
//!   and impacts of the same move draw untinted).

use std::path::Path;

use prc::hash40::hash40 as to_hash40;
use prc::ParamKind;

fn field<'a>(s: &'a prc::ParamStruct, key: &str) -> Option<&'a ParamKind> {
    let hash = to_hash40(key);
    s.0.iter().find(|(k, _)| *k == hash).map(|(_, v)| v)
}

/// The fighter's attack-arc colour (`common_effect_0`) from the game's `fighter_param.prc`.
pub fn fighter_arc_colour(data_root: &Path, fighter: &str) -> Option<[f32; 3]> {
    let root = prc::open(data_root.join("fighter/common/param/fighter_param.prc")).ok()?;
    let Some(ParamKind::List(table)) = field(&root, "fighter_param_table") else {
        return None;
    };
    let kind = to_hash40(&format!("fighter_kind_{fighter}"));
    table.0.iter().find_map(|entry| {
        let ParamKind::Struct(entry) = entry else {
            return None;
        };
        if !matches!(field(entry, "fighter_kind"), Some(ParamKind::Hash(h)) if *h == kind) {
            return None;
        }
        let channel = |name: &str| match field(entry, name) {
            Some(ParamKind::Float(v)) => Some(*v),
            _ => None,
        };
        Some([
            channel("common_effect_0_r")?,
            channel("common_effect_0_g")?,
            channel("common_effect_0_b")?,
        ])
    })
}

/// Whether the game tints this effect with the fighter's attack-arc colour.
pub fn takes_fighter_colour(effect: &str) -> bool {
    effect.to_ascii_lowercase().starts_with("sys_attack_arc")
}

/// The colour a spawn asks for, red, green, blue and opacity, or `None` when it asks for none.
/// A `LAST_EFFECT_SET_COLOR` / `_ALPHA` line after the spawn wins over the spawn's own.
pub fn spawn_colour(call: &crate::data::EffectCall) -> Option<[f32; 4]> {
    let numbers: Vec<f32> = call
        .extra_args
        .iter()
        .flatten()
        .filter_map(|arg| arg.trim().parse::<f32>().ok())
        .collect();
    let mut rgb = None;
    let mut alpha = None;
    let func = call.spawn_func.to_ascii_uppercase();
    if func.ends_with("_COLOR") && numbers.len() >= 3 {
        let n = numbers.len();
        rgb = Some([numbers[n - 3], numbers[n - 2], numbers[n - 1]]);
    } else if func.ends_with("_ALPHA") {
        alpha = numbers.last().copied();
    }
    if let Some(tint) = call.tint {
        rgb = Some(tint);
    }
    if let Some(a) = call.alpha {
        alpha = Some(a);
    }
    if rgb.is_none() && alpha.is_none() {
        return None;
    }
    let [r, g, b] = rgb.unwrap_or([1.0; 3]);
    Some([r, g, b, alpha.unwrap_or(1.0)])
}

/// The colours a fighter gives its effects in game.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FighterColours {
    /// `common_effect_0` from `fighter_param.prc`, on its attack arcs.
    pub arc: Option<[f32; 3]>,
    /// The costume's ink colour (`ink_effect_color` in `effect.prc`), which Inkling's scripts
    /// put on her effects.
    pub ink: Option<[f32; 3]>,
}

impl FighterColours {
    pub fn load(data_root: &Path, fighter: &str, slot: u8) -> Self {
        Self {
            arc: fighter_arc_colour(data_root, fighter),
            // Only Inkling, and Kirby with her ability, paint in ink.
            ink: ["inkling", "kirby"]
                .contains(&fighter)
                .then(|| ink_colour(data_root, slot))
                .flatten(),
        }
    }
}

/// The ink colour of costume `slot` (`ink_effect_color` in `fighter/common/param/effect.prc`).
pub fn ink_colour(data_root: &Path, slot: u8) -> Option<[f32; 3]> {
    let root = prc::open(data_root.join("fighter/common/param/effect.prc")).ok()?;
    let Some(ParamKind::List(colours)) = field(&root, "ink_effect_color") else {
        return None;
    };
    let ParamKind::Struct(colour) = colours.0.get(slot as usize % colours.0.len().max(1))? else {
        return None;
    };
    let channel = |name: &str| match field(colour, name) {
        Some(ParamKind::Float(v)) => Some(*v),
        _ => None,
    };
    Some([channel("r")?, channel("g")?, channel("b")?])
}

/// Whether a spawn takes the fighter's ink colour: Inkling's scripts read the ink colour from
/// the fighter and set it on the spawn with an argument-less `LAST_PARTICLE_SET_COLOR` after it
/// (the decompiled form of passing those values on). The reads can sit outside the spawn's
/// block, so the argument-less line is what marks it; only fighters with ink have a colour
/// for it (`FighterColours::ink`).
pub fn takes_ink_colour(call: &crate::data::EffectCall) -> bool {
    call.trailing
        .iter()
        .any(|line| line.contains("LAST_PARTICLE_SET_COLOR(agent)"))
}

/// The particle colour a spawn draws with (`LAST_PARTICLE_SET_COLOR` with its values), else
/// white. The shaders read it per particle, apart from the set colour.
pub fn particle_colour(call: Option<&crate::data::EffectCall>) -> [f32; 4] {
    match call.and_then(|call| call.particle_tint) {
        Some([r, g, b]) => [r, g, b, 1.0],
        None => [1.0; 4],
    }
}

/// The ink colour a spawn draws with: the fighter's, for spawns that take it.
pub fn ink(call: Option<&crate::data::EffectCall>, fighter: FighterColours) -> Option<[f32; 3]> {
    fighter.ink.filter(|_| call.is_some_and(takes_ink_colour))
}

/// The set colour the preview draws a spawn in: its own, else the fighter's arc colour for
/// attack arcs, else white.
pub fn draw_colour(
    call: Option<&crate::data::EffectCall>,
    effect: &str,
    fighter: FighterColours,
) -> [f32; 4] {
    if let Some(colour) = call.and_then(spawn_colour) {
        return colour;
    }
    match fighter.arc {
        Some([r, g, b]) if takes_fighter_colour(effect) => [r, g, b, 1.0],
        _ => [1.0; 4],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(line: &str) -> crate::data::EffectCall {
        let src = format!(
            "unsafe extern \"C\" fn effect_x(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n        {line}\n    }}\n}}"
        );
        crate::acmd::parse_effect_script(&src)
            .to_effect_calls()
            .into_iter()
            .find(|c| c.color.is_none())
            .unwrap()
    }

    #[test]
    fn a_colour_spawn_carries_its_colour() {
        // From the corpus.
        let wind = call(
            r#"macros::EFFECT_FOLLOW_COLOR(agent, Hash40::new("sys_spin_wind"), Hash40::new("top"), -1.5, 18.5, 1.5, -20, 180, 20, 0.7, true, 0.3, 0.6, 0.2);"#,
        );
        assert_eq!(spawn_colour(&wind), Some([0.3, 0.6, 0.2, 1.0]));
    }

    #[test]
    fn a_tint_line_after_the_spawn_recolours_it() {
        let arc = call(
            "macros::EFFECT_FOLLOW(agent, Hash40::new(\"sys_attack_arc\"), Hash40::new(\"top\"), 0, 10, 0, 0, 0, 0, 1, true);\n        macros::LAST_EFFECT_SET_COLOR(agent, 0.871, 0.145, 0.184);",
        );
        assert_eq!(arc.tint, Some([0.871, 0.145, 0.184]));
        assert_eq!(
            draw_colour(
                Some(&arc),
                &arc.effect_name,
                FighterColours {
                    arc: Some([0.32, 1.6, 0.4]),
                    ink: None
                }
            ),
            [0.871, 0.145, 0.184, 1.0]
        );
    }

    #[test]
    fn an_attack_arc_without_a_colour_takes_the_fighter_s() {
        let arc = call(
            r#"macros::EFFECT_FOLLOW(agent, Hash40::new("sys_attack_arc_b"), Hash40::new("top"), 1, 11, -2, 1.7, -39, -92, 1.65, true);"#,
        );
        assert_eq!(spawn_colour(&arc), None);
        let green = FighterColours {
            arc: Some([0.32, 1.6, 0.4]),
            ink: None,
        };
        assert_eq!(
            draw_colour(Some(&arc), &arc.effect_name, green),
            [0.32, 1.6, 0.4, 1.0]
        );
        // Other effects stay untinted (the probe: her dust and impacts drew at 1).
        assert_eq!(draw_colour(None, "sys_run_smoke", green), [1.0; 4]);
    }

    #[test]
    fn inkling_spawns_take_the_costume_s_ink() {
        // From her down attack, as the dump has it.
        let src = r#"unsafe extern "C" fn effect_x(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 22.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("inkling_muzzle_shot"), Hash40::new("muzzle"), -2, 0, 0, 0, 180, 0, 1, 0, 0, 0, 0, 0, 0, true);
    }
    WorkModule::get_float(agent.module_accessor, *FIGHTER_INKLING_INSTANCE_WORK_ID_FLOAT_INK_R);
    WorkModule::get_float(agent.module_accessor, -1407754940, *FIGHTER_INKLING_INSTANCE_WORK_ID_FLOAT_INK_G);
    WorkModule::get_float(agent.module_accessor, -1407754940, *FIGHTER_INKLING_INSTANCE_WORK_ID_FLOAT_INK_B);
    if macros::is_excute(agent) {
        macros::LAST_PARTICLE_SET_COLOR(agent);
    }
}"#;
        let shot = crate::acmd::parse_effect_script(src)
            .to_effect_calls()
            .into_iter()
            .next()
            .unwrap();
        assert!(takes_ink_colour(&shot));
        let orange = FighterColours {
            arc: None,
            ink: Some([0.758, 0.116, 0.04]),
        };
        assert_eq!(ink(Some(&shot), orange), Some([0.758, 0.116, 0.04]));
        // Joker's victory lines use the same form for a colour that is not ink.
        assert_eq!(ink(Some(&shot), FighterColours::default()), None);
        // A spawn with no ink line stays as it is.
        let line = call(
            r#"macros::EFFECT_FOLLOW(agent, Hash40::new("sys_attack_line_b"), Hash40::new("top"), 0, 7, -0.8, 0, 0, 0, 0.82, true);"#,
        );
        assert!(!takes_ink_colour(&line));
        assert_eq!(ink(Some(&line), orange), None);
    }

    /// Needs VISIONARY_DATA_ROOT (an extracted game); skipped otherwise.
    #[test]
    fn dark_samus_arcs_are_green_as_in_game() {
        let Some(root) = std::env::var_os("VISIONARY_DATA_ROOT") else {
            return;
        };
        assert_eq!(
            fighter_arc_colour(Path::new(&root), "samusd"),
            Some([0.32, 1.6, 0.4])
        );
        assert_eq!(
            fighter_arc_colour(Path::new(&root), "mario"),
            Some([3.0, 0.7, 0.2])
        );
        // Costume 0 is the orange Inkling, costume 1 the blue one.
        assert_eq!(
            ink_colour(Path::new(&root), 0),
            Some([0.758027, 0.115859, 0.04])
        );
        assert_eq!(
            ink_colour(Path::new(&root), 1),
            Some([0.04, 0.0608165, 0.758027])
        );
    }
}
