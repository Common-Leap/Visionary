//! Loading, saving and editing `motion_list.bin` and `motion_list.yml`.
//!
//! Ported from SSBH Editor's motion list editor. The binary layout and the YAML format follow
//! ultimate-research/motion_lib, so files round trip with yamlist and the motion_list CLI.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
pub use motion_lib::hash40::Hash40;
pub use motion_lib::mlist::{Animation, Extra, Flags, MList, Motion};

/// The game reads at most this many animations for a motion.
pub const MAX_ANIMATIONS: usize = 3;

/// One open motion list and whether it has unsaved edits.
pub struct MotionListFile {
    pub path: PathBuf,
    pub mlist: MList,
    pub changed: bool,
}

impl MotionListFile {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            path: path.to_owned(),
            mlist: load(path)?,
            changed: false,
        })
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn save(&mut self) -> anyhow::Result<()> {
        let path = self.path.clone();
        self.save_to(&path)
    }

    pub fn save_to(&mut self, path: &Path) -> anyhow::Result<()> {
        save(path, &self.mlist)?;
        self.path = path.to_owned();
        self.changed = false;
        Ok(())
    }

    pub fn reload(&mut self) -> anyhow::Result<()> {
        self.mlist = load(&self.path)?;
        self.changed = false;
        Ok(())
    }
}

/// The motion list to edit in a costume folder (`motion/body/c00`): the YAML when there is one,
/// since that is the author's own text and what a yamlist build converts, otherwise the binary.
pub fn editable_in(dir: &Path) -> Option<PathBuf> {
    ["motion_list.yml", "motion_list.yaml", "motion_list.bin"]
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

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

pub fn load(path: &Path) -> anyhow::Result<MList> {
    let bytes = std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    if is_yaml(path) {
        from_yaml(&String::from_utf8_lossy(&bytes))
    } else {
        motion_lib::read_stream(&mut Cursor::new(bytes)).context("invalid motion_list.bin")
    }
}

pub fn save(path: &Path, mlist: &MList) -> anyhow::Result<()> {
    let bytes = if is_yaml(path) {
        to_yaml(mlist)?.into_bytes()
    } else {
        to_binary(mlist)?
    };
    std::fs::write(path, bytes).with_context(|| format!("failed to write {}", path.display()))
}

pub fn to_binary(mlist: &MList) -> anyhow::Result<Vec<u8>> {
    let mut writer = Cursor::new(Vec::new());
    motion_lib::write_stream(&mut writer, mlist)?;
    Ok(writer.into_inner())
}

pub fn from_yaml(text: &str) -> anyhow::Result<MList> {
    // Labels spelled out in the file may be missing from ParamLabels.
    let value: serde_yaml::Value = serde_yaml::from_str(text).context("invalid YAML")?;
    let mut labels = Vec::new();
    collect_yaml_strings(&value, &mut labels);
    add_labels(labels);
    serde_yaml::from_value(value).context("invalid motion list YAML")
}

pub fn to_yaml(mlist: &MList) -> anyhow::Result<String> {
    let text = serde_yaml::to_string(mlist)?;
    // Labels are only written if they hash back to the same value, but check anyway: a changed
    // hash silently breaks the game.
    let round_trip = from_yaml(&text)?;
    if to_binary(&round_trip)? != to_binary(mlist)? {
        bail!("hash labels did not round trip; save as motion_list.bin instead");
    }
    Ok(text)
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

/// Adds labels whose hash is computed from the label itself. Labels that would not hash to
/// themselves cannot be added this way, which is what keeps YAML output exact.
pub fn add_labels<I: IntoIterator<Item = String>>(labels: I) {
    let map = Hash40::label_map();
    let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
    map.add_labels(labels);
}

/// Seeds the label map from Visionary's own hash → name table, keeping only names that hash to
/// their entry.
pub fn add_known_labels<'a>(labels: impl IntoIterator<Item = (&'a u64, &'a String)>) {
    add_labels(
        labels
            .into_iter()
            .filter(|(hash, name)| Hash40::new(name).0 == **hash)
            .map(|(_, name)| name.clone()),
    );
}

pub fn label(hash: Hash40) -> String {
    hash.to_label()
}

/// Parses a label or a hex hash like "0x0123456789".
pub fn parse_hash(text: &str) -> Option<Hash40> {
    let text = text.trim();
    if text.is_empty() {
        None
    } else if text.starts_with("0x") {
        Hash40::from_hex_str(text).ok()
    } else {
        add_labels([text.to_owned()]);
        Some(Hash40::new(text))
    }
}

/// Labels animations and scripts using names that can be predicted from the folder contents.
pub fn add_derived_labels<'a>(mlist: &MList, anim_names: impl IntoIterator<Item = &'a str>) {
    let mut labels: Vec<String> = anim_names.into_iter().map(|n| n.to_lowercase()).collect();
    // Scripts usually use the motion name without underscores like "game_attack11".
    for (key, motion) in &mlist.list {
        let key = label(*key);
        if key.starts_with("0x") {
            continue;
        }
        let name = key.replace('_', "");
        for prefix in ["game_", "effect_", "sound_", "expression_"] {
            for candidate in [format!("{prefix}{name}"), format!("{prefix}{key}")] {
                let hash = Hash40::new(&candidate);
                if motion.game_script == hash || motion.scripts.contains(&hash) {
                    labels.push(candidate);
                }
            }
        }
    }
    add_labels(labels);
}

pub const FLAG_NAMES: [&str; 14] = [
    "turn",
    "loop",
    "move",
    "fix_trans",
    "fix_rot",
    "fix_scale",
    "unk_40",
    "unk_80",
    "unk_100",
    "unk_200",
    "unk_400",
    "unk_800",
    "unk_1000",
    "unk_2000",
];

pub const FLAG_DESCRIPTIONS: [&str; 14] = [
    "Turn the fighter around at the end of the motion.",
    "Loop the animation.",
    "Apply the animation's root motion (trans bone) to the fighter's position.",
    "Fix the trans bone translation.",
    "Fix the trans bone rotation.",
    "Fix the trans bone scale.",
    "Unknown flag 0x40.",
    "Unknown flag 0x80.",
    "Unknown flag 0x100.",
    "Unknown flag 0x200.",
    "Unknown flag 0x400.",
    "Unknown flag 0x800.",
    "Unknown flag 0x1000.",
    "Unknown flag 0x2000.",
];

pub fn flag_mut(flags: &mut Flags, index: usize) -> Option<&mut bool> {
    Some(match index {
        0 => &mut flags.turn,
        1 => &mut flags.r#loop,
        2 => &mut flags.r#move,
        3 => &mut flags.fix_trans,
        4 => &mut flags.fix_rot,
        5 => &mut flags.fix_scale,
        6 => &mut flags.unk_40,
        7 => &mut flags.unk_80,
        8 => &mut flags.unk_100,
        9 => &mut flags.unk_200,
        10 => &mut flags.unk_400,
        11 => &mut flags.unk_800,
        12 => &mut flags.unk_1000,
        13 => &mut flags.unk_2000,
        _ => return None,
    })
}

/// Changes a motion key while preserving the entry order.
pub fn rename_motion(mlist: &mut MList, old: Hash40, new: Hash40) -> Result<(), String> {
    if old == new {
        return Ok(());
    }
    if mlist.list.contains_key(&new) {
        return Err(format!("A motion named {} already exists.", label(new)));
    }
    let list = std::mem::take(&mut mlist.list);
    mlist.list = list
        .into_iter()
        .map(|(k, v)| if k == old { (new, v) } else { (k, v) })
        .collect();
    Ok(())
}

/// Inserts a motion directly after `after` or at the end if `after` is `None`.
pub fn insert_motion_after(
    mlist: &mut MList,
    after: Option<Hash40>,
    key: Hash40,
    motion: Motion,
) -> Result<(), String> {
    if mlist.list.contains_key(&key) {
        return Err(format!("A motion named {} already exists.", label(key)));
    }
    match after.and_then(|a| mlist.list.get_index_of(&a)) {
        Some(index) => {
            let list = std::mem::take(&mut mlist.list);
            let mut new_entry = Some((key, motion));
            for (i, (k, v)) in list.into_iter().enumerate() {
                mlist.list.insert(k, v);
                if i == index {
                    if let Some((k, v)) = new_entry.take() {
                        mlist.list.insert(k, v);
                    }
                }
            }
        }
        None => {
            mlist.list.insert(key, motion);
        }
    }
    Ok(())
}

/// A key based on `base` that isn't used by any motion yet.
pub fn unique_key(mlist: &MList, base: &str) -> String {
    let mut name = base.to_owned();
    let mut i = 1;
    while mlist.list.contains_key(&Hash40::new(&name)) {
        name = format!("{base}_{i}");
        i += 1;
    }
    name
}

/// A new motion that plays `anim_name`, with defaults taken from other entries.
pub fn new_motion(mlist: &MList, anim_name: Option<&str>) -> Motion {
    // Fighter lists store cancel and intangibility frames for every motion.
    let has_extra = mlist.list.values().any(|m| m.extra.is_some());
    Motion {
        game_script: Hash40::new("dummy"),
        animations: anim_name
            .map(|name| {
                vec![Animation {
                    name: Hash40::new(&name.to_lowercase()),
                    unk: 0,
                }]
            })
            .unwrap_or_default(),
        extra: has_extra.then(Extra::default),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            scripts: vec![Hash40::new("effect_attack11"), Hash40::new("sound_attack11")],
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
        mlist.list.insert(Hash40::new("attack_11"), motion("a00attack11.nuanmb"));
        mlist.list.insert(Hash40::new("wait"), motion("a00wait1.nuanmb"));
        mlist
    }

    #[test]
    fn binary_round_trip() {
        let bytes = to_binary(&mlist()).unwrap();
        let read = motion_lib::read_stream(&mut Cursor::new(&bytes)).unwrap();
        assert_eq!(bytes, to_binary(&read).unwrap());
    }

    #[test]
    fn yaml_round_trip_with_unknown_hashes() {
        let mut mlist = mlist();
        mlist.list[0].scripts.push(Hash40(0x1234567890));
        let text = to_yaml(&mlist).unwrap();
        let read = from_yaml(&text).unwrap();
        assert_eq!(to_binary(&mlist).unwrap(), to_binary(&read).unwrap());
    }

    #[test]
    fn rename_preserves_order_and_rejects_existing() {
        let mut list = mlist();
        rename_motion(&mut list, Hash40::new("attack_11"), Hash40::new("attack_12")).unwrap();
        let keys: Vec<_> = list.list.keys().copied().collect();
        assert_eq!(vec![Hash40::new("attack_12"), Hash40::new("wait")], keys);
        assert!(rename_motion(&mut list, Hash40::new("attack_12"), Hash40::new("wait")).is_err());
    }

    #[test]
    fn insert_after_selected() {
        let mut mlist = mlist();
        let key = Hash40::new("attack_12");
        insert_motion_after(&mut mlist, Some(Hash40::new("attack_11")), key, Motion::default())
            .unwrap();
        let keys: Vec<_> = mlist.list.keys().copied().collect();
        assert_eq!(vec![Hash40::new("attack_11"), key, Hash40::new("wait")], keys);
    }

    #[test]
    fn unique_key_skips_existing() {
        assert_eq!("wait_1", unique_key(&mlist(), "wait"));
        assert_eq!("walk", unique_key(&mlist(), "walk"));
    }

    /// Point `VISIONARY_PROBE_MOTION_LIST` at a real list to check it loads and round trips:
    /// `cargo test probe_motion_list -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn probe_motion_list() {
        let Some(path) = std::env::var_os("VISIONARY_PROBE_MOTION_LIST") else {
            return;
        };
        let path = PathBuf::from(path);
        let mlist = load(&path).unwrap();
        let yaml = from_yaml(&to_yaml(&mlist).unwrap()).unwrap();
        assert_eq!(to_binary(&mlist).unwrap(), to_binary(&yaml).unwrap());
        println!("{} motions, round trip ok", mlist.list.len());
    }

    #[test]
    fn the_newer_of_bin_and_yaml_is_what_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let mut older = mlist();
        older.list[0].extra.as_mut().unwrap().cancel_frame = 11;
        save(&tmp.path().join("motion_list.bin"), &older).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        save(&tmp.path().join("motion_list.yml"), &mlist()).unwrap();
        let loaded = load_current_in(tmp.path()).unwrap();
        assert_eq!(loaded.list[0].extra.as_ref().unwrap().cancel_frame, 20);
        assert_eq!(editable_in(tmp.path()), Some(tmp.path().join("motion_list.yml")));
    }
}
