//! Reading `motion_list.bin` and `motion_list.yml`.
//!
//! The binary layout and the YAML format follow ultimate-research/motion_lib, so files written
//! by yamlist and the motion_list CLI read as they are.

use std::path::{Path, PathBuf};

use anyhow::Context;
use motion_lib::hash40::Hash40;
use motion_lib::mlist::MList;

/// The motion list the game would see in `dir`: whichever of the binary and the YAML was
/// written last, so an edit to the YAML shows before the next build converts it. A list the
/// folder only gets through a `config.json` share, or from the game, counts too.
pub fn load_current_in(dir: &Path) -> Option<MList> {
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let files = crate::costume_files::CostumeFolder::resolve(dir);
    let mut candidates: Vec<PathBuf> = ["motion_list.bin", "motion_list.yml", "motion_list.yaml"]
        .iter()
        .filter_map(|name| files.get(name).map(Path::to_path_buf))
        .collect();
    candidates.sort_by_key(|path| std::cmp::Reverse(modified(path)));
    candidates.iter().find_map(|path| load(path).ok())
}

fn is_yaml(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("yml") || e.eq_ignore_ascii_case("yaml"))
}

fn load(path: &Path) -> anyhow::Result<MList> {
    if is_yaml(path) {
        let bytes =
            std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
        from_yaml(&String::from_utf8_lossy(&bytes))
    } else {
        motion_lib::open(path).context("invalid motion_list.bin")
    }
}

fn from_yaml(text: &str) -> anyhow::Result<MList> {
    // Labels spelled out in the file may be missing from ParamLabels.
    let value: serde_yaml::Value = serde_yaml::from_str(text).context("invalid YAML")?;
    let mut labels = Vec::new();
    collect_yaml_strings(&value, &mut labels);
    let map = Hash40::label_map();
    map.lock()
        .unwrap_or_else(|e| e.into_inner())
        .add_labels(labels);
    serde_yaml::from_value(value).context("invalid motion list YAML")
}

fn collect_yaml_strings(value: &serde_yaml::Value, labels: &mut Vec<String>) {
    match value {
        serde_yaml::Value::String(s) if !s.starts_with("0x") => labels.push(s.clone()),
        serde_yaml::Value::Sequence(items) => {
            items.iter().for_each(|v| collect_yaml_strings(v, labels))
        }
        serde_yaml::Value::Mapping(map) => {
            for (k, v) in map {
                collect_yaml_strings(k, labels);
                collect_yaml_strings(v, labels);
            }
        }
        _ => (),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use motion_lib::mlist::{Animation, Extra, Flags, Motion};

    fn motion(anim: &str) -> Motion {
        Motion {
            game_script: Hash40::new("game_attack11"),
            flags: Flags {
                r#loop: true,
                ..Default::default()
            },
            blend_frames: 3,
            animations: vec![Animation {
                name: Hash40::new(anim),
                unk: 0,
            }],
            scripts: vec![
                Hash40::new("effect_attack11"),
                Hash40::new("sound_attack11"),
            ],
            extra: Some(Extra {
                xlu_start: 1,
                xlu_end: 2,
                cancel_frame: 20,
                no_stop_intp: true,
            }),
        }
    }

    fn mlist() -> MList {
        let mut mlist = MList {
            motion_path: Hash40::new("fighter/mario/motion/body"),
            ..Default::default()
        };
        mlist
            .list
            .insert(Hash40::new("attack_11"), motion("a00attack11.nuanmb"));
        mlist
            .list
            .insert(Hash40::new("wait"), motion("a00wait1.nuanmb"));
        mlist
    }

    fn binary(mlist: &MList) -> Vec<u8> {
        let mut writer = std::io::Cursor::new(Vec::new());
        motion_lib::write_stream(&mut writer, mlist).unwrap();
        writer.into_inner()
    }

    #[test]
    fn yaml_reads_names_and_unknown_hashes() {
        let mut mlist = mlist();
        mlist.list[0].scripts.push(Hash40(0x1234567890));
        let read = from_yaml(&serde_yaml::to_string(&mlist).unwrap()).unwrap();
        assert_eq!(binary(&mlist), binary(&read));
    }

    #[test]
    fn the_newer_of_bin_and_yaml_is_what_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let mut older = mlist();
        older.list[0].extra.as_mut().unwrap().cancel_frame = 11;
        motion_lib::save(tmp.path().join("motion_list.bin"), &older).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(
            tmp.path().join("motion_list.yml"),
            serde_yaml::to_string(&mlist()).unwrap(),
        )
        .unwrap();
        let loaded = load_current_in(tmp.path()).unwrap();
        assert_eq!(loaded.list[0].extra.as_ref().unwrap().cancel_frame, 20);
    }
}
