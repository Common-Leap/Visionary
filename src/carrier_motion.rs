//! Build a carrier-owned motion graph with the fighter's original motion identities.
use crate::carrier_support::pool;
use anyhow::{bail, Context};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug)]
pub struct PreparedMotions {
    pub list: motion_lib::mlist::MList,
    pub files: Vec<(String, PathBuf)>,
}

pub fn prepare(files: &BTreeMap<String, PathBuf>) -> anyhow::Result<PreparedMotions> {
    let source = files.get("motion_list.bin").context(
        "motion_list.bin is required from the costume or its vanilla dump to follow the fighter",
    )?;
    let mut list = motion_lib::open(source).context("reading fighter motion_list.bin")?;
    let mut available = BTreeMap::new();
    for (name, path) in files {
        if let Some(stem) = name.strip_suffix(".nuanmb") {
            available.insert(motion_lib::hash40::hash40(name).0, (path, true));
            available.insert(motion_lib::hash40::hash40(stem).0, (path, false));
            if let Some(raw) = stem
                .strip_prefix("0x")
                .and_then(|s| u64::from_str_radix(s, 16).ok())
            {
                available.insert(raw, (path, true));
            }
        }
    }
    let mut assigned: BTreeMap<PathBuf, usize> = BTreeMap::new();
    let mut outputs = Vec::new();
    for (kind, motion) in &mut list.list {
        // This descriptor is ultimately bound to the main fighter. Preserve its ACMD names;
        // the temporary carrier stays asleep and never plays the fighter's motion graph.
        for animation in &mut motion.animations {
            let (path, has_extension) = available.get(&animation.name.0)
                .with_context(|| format!("motion {:#x} needs missing animation {:#x}; export the complete vanilla motion folder", kind.0, animation.name.0))?;
            let index = if let Some(index) = assigned.get(*path) {
                *index
            } else {
                let index = assigned.len();
                if index >= pool::MOTION_CAPACITY {
                    bail!(
                        "motion graph exceeds the {} animation resource pool",
                        pool::MOTION_CAPACITY
                    );
                }
                assigned.insert((*path).clone(), index);
                outputs.push((pool::motion_name(index), (*path).clone()));
                index
            };
            let name = pool::motion_name(index);
            animation.name = motion_lib::hash40::hash40(if *has_extension {
                &name
            } else {
                name.strip_suffix(".nuanmb").unwrap()
            });
        }
    }
    if outputs.is_empty() {
        bail!("fighter motion list has no animation resources");
    }
    list.motion_path = motion_lib::hash40::hash40(pool::MOTION_ROOT);
    Ok(PreparedMotions {
        list,
        files: outputs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use motion_lib::{
        hash40::hash40,
        mlist::{Animation, MList, Motion},
    };

    #[test]
    fn preserves_fighter_motion_hashes_aliases_and_gameplay_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let animation = dir.path().join("a00wait1.nuanmb");
        std::fs::write(&animation, b"payload").unwrap();
        let list_path = dir.path().join("motion_list.bin");
        let mut list = MList::default();
        for (kind, name) in [("wait", "a00wait1.nuanmb"), ("custom_idle", "a00wait1")] {
            list.list.insert(
                hash40(kind),
                Motion {
                    game_script: hash40("game_attack"),
                    flags: motion_lib::mlist::Flags {
                        fix_trans: true,
                        r#move: true,
                        r#loop: true,
                        ..Default::default()
                    },
                    scripts: vec![hash40("effect_attack")],
                    animations: vec![Animation {
                        name: hash40(name),
                        unk: 3,
                    }],
                    ..Default::default()
                },
            );
        }
        motion_lib::save(&list_path, &list).unwrap();
        let files = BTreeMap::from([
            ("a00wait1.nuanmb".into(), animation),
            ("motion_list.bin".into(), list_path),
        ]);
        let prepared = prepare(&files).unwrap();
        assert_eq!(prepared.files.len(), 1);
        let wait = &prepared.list.list[&hash40("wait")];
        let alias = &prepared.list.list[&hash40("custom_idle")];
        assert_eq!(
            wait.animations[0].name,
            hash40("visionary_motion_0000.nuanmb")
        );
        assert_eq!(alias.animations[0].name, hash40("visionary_motion_0000"));
        assert_eq!(wait.animations[0].unk, 3);
        assert_eq!(wait.game_script, hash40("game_attack"));
        assert_eq!(wait.scripts, vec![hash40("effect_attack")]);
        assert!(wait.flags.fix_trans);
        assert!(wait.flags.r#loop);
        // Validate the serialized flags the game actually reads, not only the in-memory list.
        let output = dir.path().join("carrier_motion_list.bin");
        motion_lib::save(&output, &prepared.list).unwrap();
        let reloaded = motion_lib::open(&output).unwrap();
        for motion in reloaded.list.values() {
            assert_eq!(
                u16::from(motion.flags),
                u16::from(list.list[&hash40("wait")].flags)
            );
        }
        assert!(wait.flags.r#move);
        assert_eq!(prepared.list.motion_path, hash40(pool::MOTION_ROOT));
        let mut missing = files;
        missing.remove("a00wait1.nuanmb");
        assert!(prepare(&missing)
            .unwrap_err()
            .to_string()
            .contains("missing animation"));
    }
}
