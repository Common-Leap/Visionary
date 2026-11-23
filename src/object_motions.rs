//! Which of an object's motions run each of its scripts.
//!
//! The game matches a live rule by the motion an object is in, and an object's scripts are not
//! named after its motions: Ness's PK Fire runs `game_shootair` in its motion `shoot_air`. The
//! object's motion list says which motion runs which script. A script in no motion's list is one
//! the game calls by itself, with no motion to match, so it cannot go live.

use std::collections::HashMap;
use std::path::PathBuf;

const CATEGORIES: [&str; 4] = ["game", "effect", "sound", "expression"];

/// One object's scripts, by the hash of their whole name, and the motions that run each.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObjectMotions(HashMap<u64, Vec<u64>>);

impl ObjectMotions {
    /// From the first of `dirs` (the object's motion folders, most its own first) that holds a
    /// motion list. `None` when none does.
    pub fn load(dirs: impl IntoIterator<Item = PathBuf>) -> Option<Self> {
        let list = dirs
            .into_iter()
            .find_map(|dir| crate::motion_list::load_current_in(&dir))?;
        let mut scripts: HashMap<u64, Vec<u64>> = HashMap::new();
        for (motion, entry) in list.list.iter() {
            for script in std::iter::once(&entry.game_script).chain(&entry.scripts) {
                let motions = scripts.entry(script.0).or_default();
                if !motions.contains(&motion.0) {
                    motions.push(motion.0);
                }
            }
        }
        Some(Self(scripts))
    }

    /// The motions that run `script` ("shootair") of one category, or of any category for a call
    /// added to one the object has no script in.
    pub fn of(&self, category: &str, script: &str) -> Vec<u64> {
        let of = |category: &str| {
            let name = format!("{category}_{}", script.to_lowercase());
            self.0.get(&hash40::hash40(&name).0)
        };
        if let Some(motions) = of(category) {
            return motions.clone();
        }
        let mut motions: Vec<u64> = CATEGORIES
            .into_iter()
            .filter_map(of)
            .flatten()
            .copied()
            .collect();
        motions.sort_unstable();
        motions.dedup();
        motions
    }

    /// The motions that run any script of this name.
    pub fn any(&self, script: &str) -> Vec<u64> {
        self.of("", script)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use motion_lib::mlist::{MList, Motion};

    fn hash(name: &str) -> motion_lib::hash40::Hash40 {
        motion_lib::hash40::hash40(name)
    }

    fn list(motions: &[(&str, &str, &[&str])]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let list = MList {
            motion_path: hash("fighter/ness/motion/pkfire"),
            list: motions
                .iter()
                .map(|(motion, game, scripts)| {
                    let entry = Motion {
                        game_script: hash(game),
                        flags: Default::default(),
                        blend_frames: 0,
                        animations: Vec::new(),
                        scripts: scripts.iter().map(|script| hash(script)).collect(),
                        extra: None,
                    };
                    (hash(motion), entry)
                })
                .collect(),
        };
        motion_lib::save(dir.path().join("motion_list.bin"), &list).unwrap();
        dir
    }

    #[test]
    fn a_script_is_found_by_the_motions_that_run_it() {
        let dir = list(&[
            ("shoot", "game_shoot", &["effect_shoot", "sound_shoot"]),
            (
                "shoot_air",
                "game_shootair",
                &["effect_shootair", "sound_shoot"],
            ),
            ("shoot_far", "game_shootair", &["effect_shoot"]),
        ]);
        let motions = ObjectMotions::load([dir.path().to_path_buf()]).unwrap();
        let of = |category: &str, script: &str| {
            let mut found = motions.of(category, script);
            found.sort_unstable();
            found
        };
        let named = |names: &[&str]| {
            let mut hashes: Vec<u64> = names.iter().map(|name| hash40::hash40(name).0).collect();
            hashes.sort_unstable();
            hashes
        };
        assert_eq!(of("game", "shootair"), named(&["shoot_air", "shoot_far"]));
        assert_eq!(of("game", "ShootAir"), named(&["shoot_air", "shoot_far"]));
        assert_eq!(of("effect", "shootair"), named(&["shoot_air"]));
        assert_eq!(of("sound", "shoot"), named(&["shoot", "shoot_air"]));
        // A category the object has no script of: where its other scripts of that name run.
        assert_eq!(of("sound", "shootair"), named(&["shoot_air", "shoot_far"]));
        // The fireball's `effect_bound` is called by its bounce, in no motion.
        assert!(of("effect", "bound").is_empty());
        assert!(motions.any("bound").is_empty());
        assert_eq!(motions.any("shoot").len(), 3);
        assert_eq!(ObjectMotions::load([dir.path().join("missing")]), None);
    }

    /// The game's own objects: most scripts are not named after their motion, and nearly all of
    /// them are run by one.
    #[test]
    fn the_games_object_scripts_are_run_by_their_motions() {
        let (Some(root), Some(dump)) = (
            std::env::var_os("VISIONARY_DATA_ROOT").map(PathBuf::from),
            std::env::var_os("VISIONARY_SCRIPT_DUMP").map(PathBuf::from),
        ) else {
            return;
        };
        let load = |fighter: &str, name: &str| {
            ObjectMotions::load([root.join(format!("fighter/{fighter}/motion/{name}/c00"))])
                .unwrap_or_else(|| panic!("{fighter} {name}"))
        };
        let of = |fighter: &str, name: &str, category: &str, script: &str| {
            load(fighter, name).of(category, script)
        };
        let motion = |name: &str| vec![hash40::hash40(name).0];
        assert_eq!(
            of("mario", "fireball", "game", "regular"),
            motion("regular")
        );
        assert!(of("mario", "fireball", "effect", "bound").is_empty());
        assert_eq!(
            of("ness", "pkfire", "game", "shootair"),
            motion("shoot_air")
        );
        assert_eq!(
            of("ness", "yoyohead", "game", "attackhi4"),
            motion("attack_hi4")
        );
        assert_eq!(of("fox", "blaster_bullet", "game", "fly"), motion("fly"));
        let (mut run, mut same, mut alone) = (0, 0, 0);
        for module in std::fs::read_dir(&dump).unwrap().flatten() {
            let module = module.path();
            for agent in std::fs::read_dir(&module).into_iter().flatten().flatten() {
                let name = agent.file_name().into_string().unwrap();
                let Some((fighter, object)) = name.split_once('_') else {
                    continue;
                };
                let dir = root.join(format!("fighter/{fighter}/motion/{object}/c00"));
                let Some(motions) = ObjectMotions::load([dir]) else {
                    continue;
                };
                for file in std::fs::read_dir(agent.path())
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    let body = std::fs::read_to_string(file.path()).unwrap_or_default();
                    let scripts = body.lines().filter_map(|line| {
                        let name = line.strip_prefix("unsafe extern \"C\" fn ")?;
                        name.split('(').next()?.split_once('_')
                    });
                    for (category, script) in scripts {
                        let found = motions
                            .0
                            .get(&hash40::hash40(&format!("{category}_{script}")).0);
                        match found {
                            None => alone += 1,
                            Some(found) if found.contains(&hash40::hash40(script).0) => same += 1,
                            Some(_) => run += 1,
                        }
                    }
                }
            }
        }
        assert!(run > 1500 && same > 900, "{run} {same}");
        assert!(alone < 450, "{alone}");
    }
}
