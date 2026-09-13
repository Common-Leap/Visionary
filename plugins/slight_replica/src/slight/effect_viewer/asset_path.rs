//! Pure validation for live asset-bundle paths.
//!
//! This module deliberately has no Skyline or Smash dependencies so the exact plugin-side
//! allowlist can also be exercised by ordinary host tests.

/// The only SD subtree an asset-bundle message may ask the plugin to read.
pub const PAYLOAD_PREFIX: &str = "effect_viewer/live_assets/files/";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssetFamily {
    Model,
    Motion,
}

fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn is_file_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.starts_with('.')
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
}

fn is_costume(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('c') else {
        return false;
    };
    if !(2..=3).contains(&digits.len()) || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let Ok(slot) = digits.parse::<u16>() else {
        return false;
    };
    slot <= u8::MAX as u16 && value == format!("c{slot:02}")
}

fn model_file_allowed(file: &str) -> bool {
    const EXTENSIONS: &[&str] = &[
        "adjb",
        "lvd",
        "nuanmb",
        "nuhlpb",
        "numatb",
        "numdlb",
        "numshexb",
        "numshb",
        "nusktb",
        "nusrcmdlb",
        "nutexb",
        "xmb",
    ];
    file.rsplit_once('.')
        .is_some_and(|(_, extension)| EXTENSIONS.contains(&extension))
}

fn motion_file_allowed(file: &str) -> bool {
    file == "motion_list.bin"
        || matches!(file, "swing.prc" | "swingblend.prc" | "ik.prc")
        || file.ends_with(".nuanmb")
}

/// Files staged opportunistically: served when the carrier graph references them, but never
/// allowed to fail the whole snapshot when it does not. `lod.xmb` is the case in point —
/// Alucard's native tables carry no such entry, so requiring it would reject every snapshot
/// that stages it instead of loading the preview without it.
pub fn is_opportunistic_carrier_file(carrier_path: &str) -> bool {
    carrier_path
        .rsplit_once('/')
        .map(|(_, file)| file == "lod.xmb")
        .unwrap_or(false)
}

/// Validate one canonical game path and return the resource family it belongs to.
///
/// Only existing fighter costume resources are accepted. In particular, this excludes effects,
/// params, UI data, arbitrary SD paths, and generic PRCs. The callback merely serves bytes at the
/// next genuine owner load; it never mutates a live resident resource.
pub fn validate_game_path(target: &str, path: &str) -> Result<AssetFamily, &'static str> {
    if !is_identifier(target) {
        return Err("target must be a lowercase fighter identifier");
    }
    if path.is_empty()
        || path.len() > 240
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains('\\')
        || path.contains("//")
        || path
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_uppercase())
    {
        return Err("game path is not canonical lowercase ARC syntax");
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() != 6
        || parts[0] != "fighter"
        || parts[1] != target
        || !is_identifier(parts[3])
        || !is_costume(parts[4])
        || !is_file_name(parts[5])
    {
        return Err("game path must be fighter/<target>/<model|motion>/<part>/cNN/<file>");
    }
    match parts[2] {
        "model" if model_file_allowed(parts[5]) => Ok(AssetFamily::Model),
        "motion" if motion_file_allowed(parts[5]) => Ok(AssetFamily::Motion),
        "model" => Err("unsupported model asset type"),
        "motion" => Err(
            "only .nuanmb, motion_list.bin, swing.prc, swingblend.prc, and ik.prc are allowed in motion",
        ),
        _ => Err("asset family must be model or motion"),
    }
}

/// Require a payload to live at the immutable, generation-specific SD location for its game path.
///
/// A desktop replacement must never overwrite bytes that an asynchronous resource worker may
/// still be reading. Including the accepted generation in the payload path lets the desktop
/// publish a new file tree atomically and retain the old tree until the owner has retired it.
pub fn validate_payload_path(
    generation: u64,
    game_path: &str,
    payload_path: &str,
) -> Result<(), &'static str> {
    if generation == 0 {
        return Err("payload generation must be nonzero");
    }
    let expected = format!("{PAYLOAD_PREFIX}{generation}/{game_path}");
    if payload_path != expected {
        return Err("payload path must mirror the game path under effect_viewer/live_assets/files");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_supported_model_motion_and_swing_paths() {
        assert_eq!(
            validate_game_path("mario", "fighter/mario/model/body/c00/model.numdlb"),
            Ok(AssetFamily::Model)
        );
        assert_eq!(
            validate_game_path("mario", "fighter/mario/motion/body/c08/attack11.nuanmb"),
            Ok(AssetFamily::Motion)
        );
        assert_eq!(
            validate_game_path("mario", "fighter/mario/motion/body/c08/swing.prc"),
            Ok(AssetFamily::Motion)
        );
        assert_eq!(
            validate_game_path("mario", "fighter/mario/motion/body/c08/swingblend.prc"),
            Ok(AssetFamily::Motion)
        );
    }

    #[test]
    fn accepts_fighter_ik_constraints() {
        assert_eq!(
            validate_game_path("brave", "fighter/brave/motion/body/c05/ik.prc"),
            Ok(AssetFamily::Motion)
        );
    }

    #[test]
    fn rejects_traversal_wrong_target_and_unrelated_files() {
        assert!(validate_game_path(
            "mario",
            "fighter/mario/model/body/c00/../../param/fighter_param.prc"
        )
        .is_err());
        assert!(validate_game_path("mario", "fighter/luigi/model/body/c00/model.numdlb").is_err());
        assert!(validate_game_path("mario", "fighter/mario/motion/body/c00/update.prc").is_err());
        assert!(validate_game_path("mario", "effect/fighter/mario/ef_mario.eff").is_err());
    }

    #[test]
    fn only_the_lod_descriptor_is_opportunistic() {
        assert!(is_opportunistic_carrier_file(
            "assist/alucard/model/body/c00/lod.xmb"
        ));
        for path in [
            "assist/alucard/model/body/c00/model.numdlb",
            "assist/alucard/model/body/c00/model.xmb",
            "assist/alucard/motion/body/c00/swing.prc",
            "assist/alucard/model/body/c00/visionary_tex_0001.nutexb",
            "",
        ] {
            assert!(!is_opportunistic_carrier_file(path), "{path}");
        }
    }

    #[test]
    fn payload_location_is_exact_and_deterministic() {        let game = "fighter/mario/motion/body/c00/swing.prc";
        assert!(validate_payload_path(
            7,
            game,
            "effect_viewer/live_assets/files/7/fighter/mario/motion/body/c00/swing.prc"
        )
        .is_ok());
        assert!(validate_payload_path(7, game, "../swing.prc").is_err());
        assert!(
            validate_payload_path(7, game, "effect_viewer/live_assets/files/swing.prc").is_err()
        );
        assert!(validate_payload_path(
            7,
            game,
            "effect_viewer/live_assets/files/6/fighter/mario/motion/body/c00/swing.prc"
        )
        .is_err());
        assert!(validate_payload_path(0, game, "effect_viewer/live_assets/files/0/foo").is_err());
    }
}
