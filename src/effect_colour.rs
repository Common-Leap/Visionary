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

/// The colour the preview draws a spawn in: its own, else the fighter's for attack arcs, else
/// white.
pub fn draw_colour(
    call: Option<&crate::data::EffectCall>,
    effect: &str,
    fighter: Option<[f32; 3]>,
) -> [f32; 4] {
    if let Some(colour) = call.and_then(spawn_colour) {
        return colour;
    }
    match fighter {
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
            draw_colour(Some(&arc), &arc.effect_name, Some([0.32, 1.6, 0.4])),
            [0.871, 0.145, 0.184, 1.0]
        );
    }

    #[test]
    fn an_attack_arc_without_a_colour_takes_the_fighter_s() {
        let arc = call(
            r#"macros::EFFECT_FOLLOW(agent, Hash40::new("sys_attack_arc_b"), Hash40::new("top"), 1, 11, -2, 1.7, -39, -92, 1.65, true);"#,
        );
        assert_eq!(spawn_colour(&arc), None);
        let green = Some([0.32, 1.6, 0.4]);
        assert_eq!(
            draw_colour(Some(&arc), &arc.effect_name, green),
            [0.32, 1.6, 0.4, 1.0]
        );
        // Other effects stay untinted (the probe: her dust and impacts drew at 1).
        assert_eq!(draw_colour(None, "sys_run_smoke", green), [1.0; 4]);
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
    }
}
