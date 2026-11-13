//! The opponent a grab or throw holds, for the 3D preview.
//!
//! In game the grabbed fighter hangs from the thrower's `throw` bone and plays its own
//! capture or thrown animation for the move: Mario's forward throw holds the victim in
//! `thrown_f`. The preview shows a stand-in opponent the same way, so a throw is not Mario
//! throwing nothing.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::article_preview::{Flight, ObjectKind};

/// The victim's animation (its motion file name without the `eNN` prefix) for a thrower's
/// move, by motion name.
pub fn victim_motion(thrower_move: &str) -> Option<&'static str> {
    Some(match thrower_move {
        "catch_pull" | "catch_dash_pull" | "catch_turn_pull" => "capturepulledhi",
        "catch_wait" => "capturewaithi",
        "catch_attack" => "capturedamagehi",
        "catch_cut" => "capturecut",
        "throw_f" => "thrownf",
        "throw_b" => "thrownb",
        "throw_hi" => "thrownhi",
        "throw_lw" => "thrownlw",
        _ => return None,
    })
}

/// The motion file in `motion_dir` named `<three-character prefix><motion>.nuanmb`.
fn motion_file(motion_dir: &Path, motion: &str) -> Option<PathBuf> {
    std::fs::read_dir(motion_dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".nuanmb"))
                .is_some_and(|stem| stem.len() == motion.len() + 3 && stem.ends_with(motion))
        })
}

/// The opponent, posed by `motion`, as an object the preview can draw.
pub fn load(model_dir: &Path, motion_dir: &Path, motion: &str) -> Option<ObjectKind> {
    let skel = ssbh_data::skel_data::SkelData::from_file(model_dir.join("model.nusktb")).ok()?;
    let anim = ssbh_data::anim_data::AnimData::from_file(motion_file(motion_dir, motion)?).ok()?;
    Some(ObjectKind {
        name: "Opponent".to_owned(),
        model_dir: Some(model_dir.to_path_buf()),
        skel: Some(Arc::new(skel)),
        anim: Some(Arc::new(anim)),
        hitboxes: Vec::new(),
        effects: Vec::new(),
        flight: Flight::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throws_and_grabs_pose_the_victim() {
        assert_eq!(victim_motion("throw_f"), Some("thrownf"));
        assert_eq!(victim_motion("catch_wait"), Some("capturewaithi"));
        assert_eq!(victim_motion("attack_11"), None);
    }

    #[test]
    fn the_plain_thrown_animation_is_picked_over_its_variants() {
        let Some(root) = std::env::var_os("VISIONARY_DATA_ROOT") else {
            return;
        };
        let dir = Path::new(&root).join("fighter/mario/motion/body/c00");
        let file = motion_file(&dir, "thrownf").unwrap();
        assert_eq!(file.file_name().unwrap(), "e01thrownf.nuanmb");
    }
}
