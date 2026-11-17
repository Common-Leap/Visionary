//! The item a fighter holds in item moves, for the 3D preview.
//!
//! Item moves share their animations across items (`swing4`, `swingdash`), but each item has
//! its own motion kind naming it: `item_sword_swing_dash` is a dash attack with the beam sword.
//! The item's model is `item/<folder>/model/body/c00` in the data folder, and every one has a
//! `Have` joint, its grip, which the game puts in the fighter's hand.

use std::path::Path;
use std::sync::Arc;

use crate::article_preview::{Flight, ObjectKind};

/// The item folder a move holds, by the move's motion kind; `None` for moves without one.
/// The kinds name some items by an older name than their folder (`sword` is the beam sword,
/// `lipstick` the ripstick, `bat` the home-run bat).
pub fn item_for_motion(motion: &str) -> Option<&'static str> {
    let rest = motion.strip_prefix("item_")?;
    const ITEMS: [(&str, &str); 9] = [
        ("bat_", "homerunbat"),
        ("club_", "club"),
        ("deathscythe_", "deathscythe"),
        ("firebar_", "firebar"),
        ("killsword_", "killsword"),
        ("lipstick_", "ripstick"),
        ("starrod_", "starrod"),
        ("sword_", "beamsword"),
        ("hammer_", "hammer"),
    ];
    ITEMS
        .iter()
        .find(|(prefix, _)| rest.starts_with(prefix))
        .map(|(_, folder)| *folder)
}

/// The item's model and skeleton, as an object the preview draws.
pub fn load(data_root: &Path, folder: &str) -> Option<ObjectKind> {
    load_model(
        &data_root.join("item").join(folder).join("model/body/c00"),
        folder,
    )
}

/// A costume folder (`cNN`) of a model part: the fighter's own slot, else the first there is.
pub fn model_slot_dir(part: &Path, slot: u8) -> Option<std::path::PathBuf> {
    let own = part.join(format!("c{slot:02}"));
    if own.join("model.nusktb").exists() {
        return Some(own);
    }
    let mut dirs: Vec<_> = std::fs::read_dir(part)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("model.nusktb").exists())
        .collect();
    dirs.sort();
    dirs.into_iter().next()
}

/// A held model (an item or a crafted weapon) from its folder, as an object the preview draws.
pub fn load_model(model_dir: &Path, name: &str) -> Option<ObjectKind> {
    let skel = ssbh_data::skel_data::SkelData::from_file(model_dir.join("model.nusktb")).ok()?;
    let anim = ssbh_data::anim_data::AnimData::from_file(model_dir.join("model.nuanmb")).ok();
    Some(ObjectKind {
        name: name.to_owned(),
        model_dir: Some(model_dir.to_path_buf()),
        skel: Some(Arc::new(skel)),
        anim: anim.map(Arc::new),
        hitboxes: Vec::new(),
        effects: Vec::new(),
        flight: Flight::default(),
        ..ObjectKind::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_moves_name_their_item() {
        assert_eq!(item_for_motion("item_sword_swing_dash"), Some("beamsword"));
        assert_eq!(item_for_motion("item_bat_swing_4"), Some("homerunbat"));
        assert_eq!(item_for_motion("item_lipstick_swing_1"), Some("ripstick"));
        assert_eq!(item_for_motion("item_light_throw_f"), None);
        assert_eq!(item_for_motion("attack_11"), None);
    }

    #[test]
    fn every_named_item_has_a_model_with_a_grip() {
        let Some(root) = std::env::var_os("VISIONARY_DATA_ROOT") else {
            return;
        };
        if !Path::new(&root).join("item").is_dir() {
            return;
        }
        for motion in [
            "item_bat_",
            "item_club_",
            "item_deathscythe_",
            "item_firebar_",
            "item_killsword_",
            "item_lipstick_",
            "item_starrod_",
            "item_sword_",
            "item_hammer_",
        ] {
            let folder = item_for_motion(motion).unwrap();
            let kind = load(Path::new(&root), folder).unwrap_or_else(|| panic!("{folder}"));
            let skel = kind.skel.unwrap();
            assert!(
                skel.bones
                    .iter()
                    .any(|b| b.name.eq_ignore_ascii_case("have")),
                "{folder} has no Have joint"
            );
        }
    }
}
