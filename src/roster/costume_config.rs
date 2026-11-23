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
    merge(
        object,
        "new-dir-infos-base",
        bases
            .into_iter()
            .map(|(new, base)| (new, json!(base)))
            .collect(),
    );
    merge(
        object,
        "share-to-vanilla",
        vanilla
            .into_iter()
            .map(|(game, ours)| (game, json!(ours)))
            .collect(),
    );
    merge(
        object,
        "share-to-added",
        added
            .into_iter()
            .map(|(game, ours)| (game, json!(ours)))
            .collect(),
    );
    merge(
        object,
        "new-dir-files",
        files
            .into_iter()
            .map(|(info, held)| (info, json!(held)))
            .collect(),
    );
}

/// Merge `values` into the object under `key`, adding list items a list there lacks and
/// keeping every entry the config already has.
fn merge(object: &mut Map<String, Value>, key: &str, values: BTreeMap<String, Value>) {
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
}

/// Add everything `other` describes to `config`, keeping what `config` already says: two
/// `config.json` files made for one mod folder.
pub fn combine(config: &mut Value, other: Value) {
    if !config.is_object() {
        *config = json!({});
    }
    let object = config.as_object_mut().expect("made an object above");
    let Value::Object(other) = other else {
        return;
    };
    for (key, value) in other {
        match (object.get_mut(&key), value) {
            (Some(Value::Array(known)), Value::Array(more)) => {
                for item in more {
                    if !known.contains(&item) {
                        known.push(item);
                    }
                }
            }
            (Some(Value::Object(known)), Value::Object(more)) => {
                // A share or an entry both have keeps this one's word on it.
                let more = more
                    .into_iter()
                    .filter(|(name, value)| value.is_array() || !known.contains_key(name))
                    .collect();
                merge(object, &key, more);
            }
            (Some(_), _) => {}
            (None, value) => {
                object.insert(key, value);
            }
        }
    }
}

/// The costume an effect file belongs to, from its path under the mod root:
/// `effect/fighter/mario/ef_mario_c03.eff` and `effect/fighter/mario/trail_c03/x.nutexb` are
/// costume `c03` of `mario`. The game loads these only through One-Slot Effects, which finds
/// them in the costume's directory entry, so each must be listed there.
pub fn costume_effect_of(path: &str) -> Option<(String, u8)> {
    let rest = path.strip_prefix("effect/fighter/")?;
    let (fighter, rest) = rest.split_once('/')?;
    let costume = |word: &str| -> Option<u8> {
        let digits = word.strip_prefix('c')?;
        (digits.len() >= 2 && digits.bytes().all(|b| b.is_ascii_digit()))
            .then(|| digits.parse().ok())
            .flatten()
    };
    let slot = match rest.split_once('/') {
        Some((folder, _)) => costume(folder.strip_prefix("trail_")?)?,
        None => costume(
            rest.strip_suffix(".eff")?
                .strip_prefix(&format!("ef_{fighter}_"))?,
        )?,
    };
    Some((fighter.to_owned(), slot))
}

/// List `paths` (from the mod root) in costume `costume` of `fighter`'s directory entry, the
/// `new-dir-files` shape One-Slot Effects reads.
pub fn declare(config: &mut Value, fighter: &str, costume: u8, paths: &[String]) {
    if !config.is_object() {
        *config = json!({});
    }
    let object = config.as_object_mut().expect("made an object above");
    merge(
        object,
        "new-dir-files",
        BTreeMap::from([(
            format!("fighter/{}/c{costume:02}", fighter.to_ascii_lowercase()),
            json!(paths),
        )]),
    );
}

/// Every costume effect file under `mod_root` that its `config.json` does not list in the
/// costume's directory entry. The game crashes when that costume is picked.
pub fn undeclared_costume_effects(mod_root: &std::path::Path) -> Vec<String> {
    fn walk(dir: &std::path::Path, root: &std::path::Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, found);
            } else if let Ok(relative) = path.strip_prefix(root) {
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut files = Vec::new();
    walk(&mod_root.join("effect"), mod_root, &mut files);
    files.sort();
    let config: Value = std::fs::read(mod_root.join("config.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null);
    files
        .into_iter()
        .filter(|path| {
            let Some((fighter, costume)) = costume_effect_of(path) else {
                return false;
            };
            let listed = config["new-dir-files"][format!("fighter/{fighter}/c{costume:02}")]
                .as_array()
                .is_some_and(|list| list.iter().any(|item| item.as_str() == Some(path)));
            !listed
        })
        .collect()
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

    /// A costume's own effect file and trail folder count as unlisted until the costume's
    /// entry lists them; the base file never needs one. Both halves run on one folder.
    #[test]
    fn a_costume_effect_file_is_unlisted_until_its_costume_lists_it() {
        assert_eq!(
            costume_effect_of("effect/fighter/bayonetta/ef_bayonetta_c01.eff"),
            Some(("bayonetta".to_owned(), 1))
        );
        assert_eq!(
            costume_effect_of("effect/fighter/mario/trail_c12/tex.nutexb"),
            Some(("mario".to_owned(), 12))
        );
        for plain in [
            "effect/fighter/bayonetta/ef_bayonetta.eff",
            "effect/fighter/bayonetta/ef_bayonetta_cx1.eff",
            "effect/fighter/bayonetta/trail/tex.nutexb",
            "effect/fighter/bayonetta/model/c01/a.bfres",
            "fighter/bayonetta/model/body/c01/model.numdlb",
        ] {
            assert_eq!(costume_effect_of(plain), None, "{plain}");
        }

        let dir = tempfile::tempdir().unwrap();
        let costume = "effect/fighter/bayonetta/ef_bayonetta_c01.eff";
        let trail = "effect/fighter/bayonetta/trail_c02/tex.nutexb";
        for file in ["effect/fighter/bayonetta/ef_bayonetta.eff", costume, trail] {
            let path = dir.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }
        assert_eq!(undeclared_costume_effects(dir.path()), [costume, trail]);

        // Listed under another costume is still unlisted.
        let mut config = json!({"new-dir-files": {"kept": ["as it was"]}});
        declare(&mut config, "Bayonetta", 2, &[costume.to_owned()]);
        std::fs::write(dir.path().join("config.json"), config.to_string()).unwrap();
        assert_eq!(undeclared_costume_effects(dir.path()), [costume, trail]);

        declare(&mut config, "bayonetta", 1, &[costume.to_owned()]);
        declare(&mut config, "bayonetta", 2, &[trail.to_owned()]);
        std::fs::write(dir.path().join("config.json"), config.to_string()).unwrap();
        assert!(undeclared_costume_effects(dir.path()).is_empty());
        assert_eq!(
            strings(&config["new-dir-files"]["fighter/bayonetta/c02"]),
            [costume, trail]
        );
        assert_eq!(config["new-dir-files"]["kept"][0], "as it was");
    }

    /// Two configs for one mod folder add up: the export's entry for a costume's effect file and
    /// what the project's own files say of theirs.
    #[test]
    fn two_configs_for_one_mod_add_up() {
        let mut ours = json!({});
        declare(
            &mut ours,
            "pacman",
            40,
            &["effect/fighter/pacman/ef_pacman_c40.eff".to_owned()],
        );
        combine(
            &mut ours,
            json!({
                "new-dir-infos": ["fighter/pacman/c40"],
                "new-dir-infos-base": {"fighter/pacman/c40/cmn": "fighter/pacman/c00/cmn"},
                "share-to-vanilla": {"fighter/pacman/model/body/c00/model.xmb":
                    ["fighter/pacman/model/body/c40/model.xmb"]},
                "new-dir-files": {
                    "fighter/pacman/c40": ["fighter/pacman/model/body/c40/model.numdlb"],
                    "fighter/pacman/c41": ["fighter/pacman/model/body/c41/model.numdlb"],
                },
            }),
        );
        assert_eq!(
            strings(&ours["new-dir-files"]["fighter/pacman/c40"]),
            [
                "effect/fighter/pacman/ef_pacman_c40.eff",
                "fighter/pacman/model/body/c40/model.numdlb",
            ]
        );
        assert_eq!(strings(&ours["new-dir-infos"]), ["fighter/pacman/c40"]);
        assert_eq!(
            ours["new-dir-infos-base"]["fighter/pacman/c40/cmn"],
            "fighter/pacman/c00/cmn"
        );
        assert!(ours["share-to-vanilla"]["fighter/pacman/model/body/c00/model.xmb"].is_array());
        // Once more changes nothing.
        let once = ours.clone();
        combine(&mut ours, once.clone());
        assert_eq!(ours, once);
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
