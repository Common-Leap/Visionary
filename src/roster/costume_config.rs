//! The `config.json` ARCropolis needs for costumes the game does not have.
//!
//! The game loads a costume through a directory entry listing its files: `fighter/mario/c00`
//! for the costume itself, with `camera` and `cmn` under it, and beside it one for each of the
//! victory, movie and Kirby-hat sets. A costume past the last one the game ships has no such
//! entries, so a mod has to describe them: which entries to add (`new-dir-infos`), which of the
//! base costume's sub-entries they borrow (`new-dir-infos-base`), every file each holds
//! (`new-dir-files`), and for each file the mod does not carry, the game file that stands in
//! for it (`share-to-vanilla`, or `share-to-added` for animations, cameras and sounds).
//!
//! What a base costume's entries hold is the game's own table, kept in
//! `assets/costume_dirs.dat` (a zip of one text file): per fighter, per entry, the file list
//! with the costume number written `@`. A file listed under another costume's number is one
//! the game itself shares, and stays as written.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::sync::OnceLock;

use serde_json::{json, Map, Value};

static TABLE: &[u8] = include_bytes!("../../assets/costume_dirs.dat");

/// One directory entry of a base costume.
struct Entry {
    /// `.` for the costume itself, else the set beside it (`camera`, `kirbycopy`, ...).
    group: String,
    costume: u8,
    /// Sub-entries a new costume borrows from this one.
    subdirs: Vec<String>,
    /// Paths with the costume number as `@`: under the fighter's folder, or from the root
    /// when they start with `/`.
    files: Vec<String>,
}

fn table() -> &'static HashMap<String, Vec<Entry>> {
    static PARSED: OnceLock<HashMap<String, Vec<Entry>>> = OnceLock::new();
    PARSED.get_or_init(|| {
        let mut text = String::new();
        if let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(TABLE)) {
            if let Ok(mut file) = archive.by_index(0) {
                let _ = file.read_to_string(&mut text);
            }
        }
        let mut fighters: HashMap<String, Vec<Entry>> = HashMap::new();
        let mut fighter = String::new();
        for line in text.lines() {
            let costume_of = |word: &str| word.trim_start_matches('c').parse::<u8>().unwrap_or(0);
            if let Some(name) = line.strip_prefix("F ") {
                fighter = name.to_owned();
                continue;
            }
            let entries = fighters.entry(fighter.clone()).or_default();
            let words: Vec<&str> = line.split(' ').collect();
            match words.as_slice() {
                ["D", group, costume, subdirs] => entries.push(Entry {
                    group: (*group).to_owned(),
                    costume: costume_of(costume),
                    subdirs: subdirs
                        .split(',')
                        .filter(|sub| !sub.is_empty())
                        .map(str::to_owned)
                        .collect(),
                    files: Vec::new(),
                }),
                // The same lists as another costume's entry of this set.
                ["=", group, costume, like] => {
                    let like = costume_of(like);
                    let Some(same) = entries
                        .iter()
                        .find(|entry| entry.group == *group && entry.costume == like)
                    else {
                        continue;
                    };
                    let copy = Entry {
                        group: (*group).to_owned(),
                        costume: costume_of(costume),
                        subdirs: same.subdirs.clone(),
                        files: same.files.clone(),
                    };
                    entries.push(copy);
                }
                _ => {
                    if let Some(entry) = entries.last_mut() {
                        entry.files.push(line.to_owned());
                    }
                }
            }
        }
        fighters
    })
}

/// Whether the game has a costume table for `fighter` (it does for every playable one).
pub fn known(fighter: &str) -> bool {
    table().contains_key(&fighter.to_ascii_lowercase())
}

/// `path` with its first costume number replaced by `costume`.
fn recostumed(path: &str, costume: u8) -> String {
    let bytes = path.as_bytes();
    let at = (0..bytes.len().saturating_sub(2))
        .find(|&i| bytes[i] == b'c' && bytes[i + 1] == b'0' && bytes[i + 2].is_ascii_digit());
    match at {
        Some(at) => format!("{}c{costume:02}{}", &path[..at], &path[at + 3..]),
        None => path.to_owned(),
    }
}

/// Add what `fighter`'s new costumes need to `config`. Each of `costumes` is (the new costume,
/// the base costume it stands on); `provided` is every file the mod carries, from its root.
pub fn add(config: &mut Value, fighter: &str, costumes: &[(u8, u8)], provided: &BTreeSet<String>) {
    let fighter = fighter.to_ascii_lowercase();
    let Some(entries) = table().get(&fighter) else {
        return;
    };
    if !config.is_object() {
        *config = json!({});
    }
    let root = format!("fighter/{fighter}");
    let mut infos: Vec<String> = Vec::new();
    let mut bases: BTreeMap<String, String> = BTreeMap::new();
    let mut files: Vec<(String, Vec<String>)> = Vec::new();
    let mut vanilla: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut added: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (new, base) in costumes {
        let (own, from) = (format!("c{new:02}"), format!("c{base:02}"));
        let mut listed: BTreeSet<String> = BTreeSet::new();
        for entry in entries.iter().filter(|entry| entry.costume == *base) {
            let (info, base_info) = if entry.group == "." {
                (format!("{root}/{own}"), format!("{root}/{from}"))
            } else {
                (
                    format!("{root}/{}/{own}", entry.group),
                    format!("{root}/{}/{from}", entry.group),
                )
            };
            infos.push(info.clone());
            for sub in &entry.subdirs {
                bases.insert(format!("{info}/{sub}"), format!("{base_info}/{sub}"));
            }
            let mut held = Vec::new();
            for file in &entry.files {
                let game = match file.strip_prefix('/') {
                    Some(rooted) => rooted.replace('@', &from),
                    None => format!("{root}/{}", file.replace('@', &from)),
                };
                let ours = recostumed(&game, *new);
                if !held.contains(&ours) {
                    held.push(ours.clone());
                }
                listed.insert(ours.clone());
                if provided.contains(&ours) {
                    continue;
                }
                // A texture is never borrowed into a folder the mod fills itself: its model
                // names its own.
                let folder = ours.rsplit_once('/').map_or("", |(folder, _)| folder);
                if ours.ends_with(".nutexb")
                    && provided
                        .iter()
                        .any(|path| path.rsplit_once('/').is_some_and(|(dir, _)| dir == folder))
                {
                    continue;
                }
                let animated = game.contains("motion/")
                    || game.contains("camera/")
                    || game.starts_with("sound/bank/fighter");
                let shares = if animated { &mut added } else { &mut vanilla };
                let targets = shares.entry(game).or_default();
                if !targets.contains(&ours) {
                    targets.push(ours);
                }
            }
            files.push((info, held));
        }
        // Files of the mod's own that the game's costume never had, and the costume's effects.
        let main = format!("{root}/{own}");
        let camera = format!("camera/fighter/{fighter}/{own}/");
        let effect = format!("effect/fighter/{fighter}/ef_{fighter}_{own}");
        let marker = format!("/{own}/");
        for path in provided {
            if listed.contains(path) {
                continue;
            }
            let info = if path.starts_with(&camera) {
                format!("{main}/camera")
            } else if path.starts_with(&effect)
                || (path.contains(&marker) && path.starts_with(&format!("{root}/")))
            {
                main.clone()
            } else {
                continue;
            };
            match files.iter_mut().find(|(known, _)| *known == info) {
                Some((_, held)) => held.push(path.clone()),
                None => files.push((info, vec![path.clone()])),
            }
        }
    }

    let object = config.as_object_mut().expect("made an object above");
    let list = object.entry("new-dir-infos").or_insert_with(|| json!([]));
    if let Some(list) = list.as_array_mut() {
        for info in infos {
            if !list.iter().any(|known| known == &info) {
                list.push(json!(info));
            }
        }
    }
    let mut merge = |key: &str, values: BTreeMap<String, Value>| {
        let Some(map) = object
            .entry(key)
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
        else {
            return;
        };
        for (name, value) in values {
            match (map.get_mut(&name).and_then(Value::as_array_mut), value) {
                (Some(known), Value::Array(more)) => {
                    for item in more {
                        if !known.contains(&item) {
                            known.push(item);
                        }
                    }
                }
                (_, value) => {
                    map.insert(name, value);
                }
            }
        }
    };
    merge(
        "new-dir-infos-base",
        bases
            .into_iter()
            .map(|(new, base)| (new, json!(base)))
            .collect(),
    );
    merge(
        "share-to-vanilla",
        vanilla
            .into_iter()
            .map(|(game, ours)| (game, json!(ours)))
            .collect(),
    );
    merge(
        "share-to-added",
        added
            .into_iter()
            .map(|(game, ours)| (game, json!(ours)))
            .collect(),
    );
    merge(
        "new-dir-files",
        files
            .into_iter()
            .map(|(info, held)| (info, json!(held)))
            .collect(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(value: &Value) -> Vec<&str> {
        value
            .as_array()
            .map(|list| list.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    }

    /// Lucina's ninth costume standing on her first, with only a model file of its own.
    #[test]
    fn a_new_costume_lists_every_file_and_borrows_the_ones_it_lacks() {
        assert!(known("lucina") && !known("nobody"));
        let provided = BTreeSet::from([
            "fighter/lucina/model/body/c08/model.numdlb".to_owned(),
            "fighter/lucina/model/body/c08/my_own.nutexb".to_owned(),
            "effect/fighter/lucina/ef_lucina_c08.eff".to_owned(),
        ]);
        let mut config = json!({"new-dir-files": {"kept": ["as it was"]}});
        add(&mut config, "Lucina", &[(8, 0)], &provided);

        let infos = strings(&config["new-dir-infos"]);
        for info in [
            "fighter/lucina/c08",
            "fighter/lucina/camera/c08",
            "fighter/lucina/kirbycopy/c08",
        ] {
            assert!(infos.contains(&info), "{info} is missing from {infos:?}");
        }
        assert_eq!(
            config["new-dir-infos-base"]["fighter/lucina/c08/cmn"],
            "fighter/lucina/c00/cmn"
        );
        assert_eq!(
            config["new-dir-infos-base"]["fighter/lucina/c08/camera"],
            "fighter/lucina/c00/camera"
        );

        let held = strings(&config["new-dir-files"]["fighter/lucina/c08"]);
        // The game's table has 464 files for this costume. 32 of them are files it shares from
        // another costume under a name this one also has, so they land on the same new path;
        // the mod's two extras join the 432 left.
        assert_eq!(held.len(), 432 + 2);
        assert!(held.contains(&"fighter/lucina/model/body/c08/model.numdlb"));
        assert!(held.contains(&"fighter/lucina/model/body/c08/my_own.nutexb"));
        assert!(held.contains(&"effect/fighter/lucina/ef_lucina_c08.eff"));
        assert!(held.contains(&"sound/bank/fighter/se_lucina_c08.nus3audio"));
        assert_eq!(config["new-dir-files"]["kept"][0], "as it was");

        // Carried by the mod: not borrowed. Not carried: borrowed from the base costume.
        assert!(config["share-to-vanilla"]
            .get("fighter/lucina/model/body/c00/model.numdlb")
            .is_none());
        assert_eq!(
            strings(&config["share-to-vanilla"]["fighter/lucina/model/body/c00/model.numshb"]),
            ["fighter/lucina/model/body/c08/model.numshb"]
        );
        // No texture is borrowed into a folder the mod fills itself.
        assert!(config["share-to-vanilla"]
            .as_object()
            .unwrap()
            .iter()
            .all(|(game, _)| !(game.contains("model/body/") && game.ends_with(".nutexb"))));
        // Animations and sounds are borrowed as added files.
        assert_eq!(
            strings(&config["share-to-added"]["sound/bank/fighter/se_lucina_c00.nus3audio"]),
            ["sound/bank/fighter/se_lucina_c08.nus3audio"]
        );
        assert!(config["share-to-added"]
            .as_object()
            .unwrap()
            .keys()
            .any(|game| game.starts_with("fighter/lucina/motion/body/c00/")));
    }

    #[test]
    fn a_second_costume_adds_to_the_same_config_without_repeats() {
        let mut config = json!({});
        add(&mut config, "mario", &[(8, 0), (9, 1)], &BTreeSet::new());
        let once = config.clone();
        add(&mut config, "mario", &[(8, 0), (9, 1)], &BTreeSet::new());
        assert_eq!(config, once);
        assert!(strings(&config["new-dir-infos"]).contains(&"fighter/mario/c09"));
        assert_eq!(
            config["new-dir-infos-base"]["fighter/mario/c09/cmn"],
            "fighter/mario/c01/cmn"
        );
    }

    #[test]
    fn only_the_first_costume_number_in_a_path_changes() {
        assert_eq!(
            recostumed("fighter/a/model/body/c06/x_c00.bin", 12),
            "fighter/a/model/body/c12/x_c00.bin"
        );
        assert_eq!(
            recostumed("sound/bank/fighter/se_a_c03.nus3bank", 8),
            "sound/bank/fighter/se_a_c08.nus3bank"
        );
        assert_eq!(recostumed("no/costume/here", 8), "no/costume/here");
    }
}
