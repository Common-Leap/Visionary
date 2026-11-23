//! Which ACMD scripts a motion actually runs, read from `motion_list.bin`.
//!
//! Script names used to be GUESSED from the motion's name: strip the underscores, prefix
//! `game_`, and patch a handful of known exceptions (`attack_s4_s` → `game_attacks4`). That holds
//! for vanilla motions and fails for nearly every custom one. Shy Guy's reverse forward smash is
//! the motion `attack_s4_s_reverse`, whose scripts are `game_attacks4reverse` & co. — no rule
//! derives that, and the motion is not in ParamLabels either, so it showed up as a raw hash with
//! no scripts at all.
//!
//! The motion list already says the answer: every entry stores the hashes of its game script and
//! its effect/sound/expression scripts. This module turns those hashes back into names, using a
//! dictionary built from ParamLabels plus every identifier-like string in the moveset itself (its
//! Rust source when one is linked, and its compiled `.nro` — smashline registers scripts by
//! string, so the names are in the binary even when the source is not available).
//!
//! Only motions whose real script differs from the guessed one are registered, so vanilla lookups
//! — including the GitHub dump paths derived from motion names — behave exactly as before.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

/// The four ACMD categories a motion-list entry names scripts for.
pub const SCRIPT_PREFIXES: [&str; 4] = ["game_", "effect_", "sound_", "expression_"];

/// motion name (lowercase) → script suffix the motion list says it runs (`attacks4reverse`).
static REGISTRY: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn hash40(text: &str) -> u64 {
    hash40::hash40(text).0
}

/// The script suffix the motion list assigned to `motion`, when it differs from the guess.
pub fn registered_suffix(motion: &str) -> Option<String> {
    REGISTRY
        .read()
        .ok()?
        .get(&motion.to_ascii_lowercase())
        .cloned()
}

pub fn register(motion: &str, suffix: &str) {
    if let Ok(mut map) = REGISTRY.write() {
        map.insert(motion.to_ascii_lowercase(), suffix.to_ascii_lowercase());
    }
}

/// The suffix the name-based guess produces for a motion (`attack_air_n` → `attackairn`).
fn guessed_suffix(motion: &str) -> String {
    motion
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Resolve a motion's script hashes to the suffix its scripts share, e.g. `attacks4reverse`.
///
/// `script_hashes` is the entry's game script followed by its other scripts. The first hash
/// that resolves to a `<category>_<suffix>` name wins; the game script is listed first because
/// every motion that runs ACMD has one, while effect/sound/expression are optional.
pub fn script_suffix(
    motion: &str,
    script_hashes: &[u64],
    names: &HashMap<u64, String>,
) -> Option<String> {
    // Candidates derived from the motion itself resolve even with an empty dictionary — that
    // is what keeps every vanilla motion working without any labels at all.
    let mut derived: Vec<String> = crate::acmd::move_script_aliases_guess(motion)
        .iter()
        .map(|alias| guessed_suffix(alias))
        .collect();
    derived.dedup();
    for &hash in script_hashes {
        if hash == 0 {
            continue;
        }
        for prefix in SCRIPT_PREFIXES {
            for suffix in &derived {
                if hash40(&format!("{prefix}{suffix}")) == hash {
                    return Some(suffix.clone());
                }
            }
        }
        if let Some(name) = names.get(&hash) {
            let lower = name.to_ascii_lowercase();
            for prefix in SCRIPT_PREFIXES {
                if let Some(suffix) = lower.strip_prefix(prefix) {
                    if !suffix.is_empty() {
                        return Some(suffix.to_string());
                    }
                }
            }
        }
    }
    None
}

/// A name for a motion no dictionary knows, taken from the scripts it runs: `specialnshoot` for
/// a motion whose game script is `game_specialnshoot`. Not the motion's real name (underscores
/// cannot be recovered), but it reads, and because a motion's ACMD name IS this suffix, every
/// script lookup derived from it lands on the right functions with no registration needed.
pub fn name_from_scripts(script_hashes: &[u64], names: &HashMap<u64, String>) -> Option<String> {
    script_hashes.iter().find_map(|hash| {
        let lower = names.get(hash)?.to_ascii_lowercase();
        SCRIPT_PREFIXES.iter().find_map(|prefix| {
            lower
                .strip_prefix(prefix)
                .filter(|suffix| !suffix.is_empty())
                .map(str::to_string)
        })
    })
}

/// Every name a yamlist `motion_list.yml` spells out: motion names, their game/effect/sound/
/// expression scripts, and animation file names, keyed by hash40.
///
/// Mod authors edit the motion list as YAML and convert it to `motion_list.bin`, so the YAML is
/// usually sitting next to the binary in their romfs. When it is, it is the ground truth for
/// everything this module otherwise has to reconstruct from hashes. yamlist's layout is fixed —
/// `list:` then one two-space-indented key per motion, with `game_script: x`, `- name: x` under
/// `animations`, and `- x` under `scripts` — so a line reader is enough.
pub fn names_from_motion_list_yml(text: &str) -> HashMap<u64, String> {
    let mut out = HashMap::new();
    let mut add = |name: &str| {
        let name = name.trim().trim_matches(['"', '\'']);
        if !name.is_empty() && !name.contains(' ') {
            out.entry(hash40(name)).or_insert_with(|| name.to_string());
        }
    };
    let mut in_list = false;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim();
        if indent == 0 {
            in_list = trimmed == "list:";
            continue;
        }
        if !in_list {
            continue;
        }
        if indent == 2 {
            // `  attack_s4_s_reverse:` — a motion key.
            if let Some(motion) = trimmed.strip_suffix(':') {
                add(motion);
            }
        } else if let Some(script) = trimmed.strip_prefix("game_script:") {
            add(script);
        } else if let Some(animation) = trimmed.strip_prefix("- name:") {
            add(animation);
        } else if let Some(item) = trimmed.strip_prefix("- ") {
            // Script list entries (`- effect_attacks4reverse`). Skip anything else in list
            // form by requiring a script prefix.
            if SCRIPT_PREFIXES.iter().any(|p| item.starts_with(p)) {
                add(item);
            }
        }
    }
    out
}

/// The best name available for a motion, most trustworthy first:
///
/// 1. a dictionary hit (ParamLabels, or a string the moveset itself spells out),
/// 2. the exact name recovered from an animation file ([`name_from_animation`]),
/// 3. the suffix of the scripts it runs ([`name_from_scripts`]),
/// 4. an animation file's stem, underscores missing ([`animation_stem`]).
///
/// `None` only when the motion has no name, no resolvable script and no animation at all.
pub fn motion_name(
    motion_hash: u64,
    script_hashes: &[u64],
    animation_hashes: &[u64],
    names: &HashMap<u64, String>,
) -> Option<String> {
    if let Some(name) = names.get(&motion_hash) {
        return Some(name.clone());
    }
    let animations: Vec<String> = animation_hashes
        .iter()
        .filter_map(|hash| names.get(hash).cloned())
        .collect();
    name_from_animation(motion_hash, &animations)
        .or_else(|| name_from_scripts(script_hashes, names))
        .or_else(|| animations.iter().find_map(|a| animation_stem(a)))
}

/// Recover a motion's exact name from one of its animation file names.
///
/// Animations are named after their motion with the underscores dropped and a slot prefix
/// added: motion `walk_fall_r` plays `a04walkfallr.nuanmb`. The prefix is always one letter and
/// two digits. Putting the underscores back is a search over the gaps between characters, and the
/// motion's own hash says which placement is right — so this returns the REAL name or nothing.
///
/// Stems above [`MAX_UNDERSCORE_SEARCH`] characters are not searched (the space doubles per
/// character); [`animation_stem`] still offers them as a readable fallback.
pub fn name_from_animation(motion_hash: u64, animation_names: &[String]) -> Option<String> {
    animation_names.iter().find_map(|animation| {
        let stem = animation_stem(animation)?;
        if hash40(&stem) == motion_hash {
            return Some(stem);
        }
        let chars: Vec<char> = stem.chars().collect();
        let gaps = chars.len().checked_sub(1)?;
        if gaps == 0 || chars.len() > MAX_UNDERSCORE_SEARCH {
            return None;
        }
        // The hash also encodes the length, so only placements with exactly the missing
        // number of underscores can match.
        let want = (motion_hash >> 32) as usize;
        let missing = want.checked_sub(chars.len())?;
        if missing == 0 || missing > gaps {
            return None;
        }
        // Choose `missing` of the gaps: C(23, 4) is under 9k candidates, where every bitmask of
        // a 24-character stem would be 8M.
        let mut picks: Vec<usize> = (0..missing).collect();
        let mut name = String::with_capacity(want);
        loop {
            name.clear();
            let mut next = picks.iter().peekable();
            for (i, c) in chars.iter().enumerate() {
                if i > 0 && next.peek().is_some_and(|&&gap| gap == i - 1) {
                    name.push('_');
                    next.next();
                }
                name.push(*c);
            }
            if hash40(&name) == motion_hash {
                return Some(name);
            }
            // Advance to the next combination in lexicographic order.
            let mut slot = missing;
            loop {
                if slot == 0 {
                    return None;
                }
                slot -= 1;
                if picks[slot] < gaps - missing + slot {
                    break;
                }
            }
            picks[slot] += 1;
            for later in slot + 1..missing {
                picks[later] = picks[later - 1] + 1;
            }
        }
    })
}

/// Longest animation stem [`name_from_animation`] searches for underscore placements.
pub const MAX_UNDERSCORE_SEARCH: usize = 24;

/// `a04walkfallr.nuanmb` → `walkfallr`: the animation's motion name minus its underscores.
pub fn animation_stem(animation: &str) -> Option<String> {
    let file = animation.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
    let stem = file.strip_suffix(".nuanmb").unwrap_or(&file);
    let bytes = stem.as_bytes();
    let prefixed = bytes.len() > 3
        && bytes[0].is_ascii_lowercase()
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit();
    let rest = if prefixed { &stem[3..] } else { stem };
    (!rest.is_empty()).then(|| rest.to_string())
}

/// Record the motion list's script for `motion` when it is not what the name-based guess (and
/// its known aliases) would already find. Returns whether anything was registered.
pub fn register_if_unguessable(motion: &str, suffix: &str) -> bool {
    let guessable = crate::acmd::move_script_aliases_guess(motion)
        .iter()
        .any(|alias| guessed_suffix(alias) == suffix);
    if guessable {
        return false;
    }
    register(motion, suffix);
    true
}

/// Every identifier-like string (`[a-z0-9_]`, 3..=96 chars) in `bytes`, keyed by its hash40.
fn harvest_identifiers(bytes: &[u8], out: &mut HashMap<u64, String>) {
    let mut start: Option<usize> = None;
    for (i, &b) in bytes.iter().chain(std::iter::once(&0u8)).enumerate() {
        let ident = b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_';
        match (ident, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                let len = i - s;
                if (3..=96).contains(&len) {
                    // Safe: every byte in the run is ASCII.
                    let text = std::str::from_utf8(&bytes[s..i]).unwrap_or_default();
                    out.entry(hash40(text)).or_insert_with(|| text.to_string());
                }
                start = None;
            }
            _ => {}
        }
    }
}

/// Names a moveset spells out itself: string contents of its Rust source and of any compiled
/// `.nro` plugin under `roots`. Motion and script names a mod adds are never in ParamLabels, but
/// they are always in one of these — smashline registers scripts and `hash40("…")` names motions
/// by string.
pub fn moveset_names(source_files: &[PathBuf], roots: &[PathBuf]) -> HashMap<u64, String> {
    let mut out = HashMap::new();
    for file in source_files {
        if let Ok(text) = std::fs::read(file) {
            harvest_identifiers(&text, &mut out);
        }
    }
    for root in roots {
        for nro in find_nros(root) {
            if let Ok(bytes) = std::fs::read(&nro) {
                harvest_identifiers(&bytes, &mut out);
            }
        }
    }
    out
}

/// `.nro` plugins a mod root ships: at its top level (a loose romfs), or under the usual
/// `skyline/plugins` / `atmosphere/contents/<title>/romfs/skyline/plugins` layout. Bounded depth
/// so pointing a root at a whole dump cannot turn into a full disk walk.
fn find_nros(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                // Only descend where plugins live; never into fighter/effect/sound data.
                let plugin_dir = matches!(
                    name.as_str(),
                    "skyline" | "plugins" | "atmosphere" | "contents" | "romfs" | "exefs"
                ) || name.starts_with("01006a800016e000");
                if depth > 0 && plugin_dir {
                    walk(&path, depth - 1, out);
                }
            } else if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("nro"))
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, 6, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shy Guy's reverse fsmash: a motion no name rule can map to its scripts.
    #[test]
    fn a_custom_motion_resolves_through_its_script_hash() {
        let mut names = HashMap::new();
        harvest_identifiers(
            br#".game_acmd("game_attacks4reverse", game_attacks4reverse, Priority::Default)
                fighter.is_motion(hash40("attack_s4_s_reverse"))"#,
            &mut names,
        );
        let motion = names
            .get(&hash40("attack_s4_s_reverse"))
            .cloned()
            .expect("motion name recovered from source text");
        let suffix = script_suffix(&motion, &[hash40("game_attacks4reverse")], &names);
        assert_eq!(suffix.as_deref(), Some("attacks4reverse"));
        // The name-based guess would have looked for `game_attacks4sreverse`.
        assert_ne!(guessed_suffix(&motion), "attacks4reverse");
    }

    /// Vanilla motions resolve without any dictionary and are never registered, so their
    /// existing lookups (and GitHub dump paths) are untouched.
    #[test]
    fn vanilla_motions_need_no_dictionary_and_stay_unregistered() {
        let empty = HashMap::new();
        let s4 = script_suffix("attack_s4_s", &[hash40("game_attacks4")], &empty);
        assert_eq!(s4.as_deref(), Some("attacks4"));
        assert!(!register_if_unguessable("attack_s4_s", "attacks4"));
        let air = script_suffix("attack_air_n", &[hash40("game_attackairn")], &empty);
        assert_eq!(air.as_deref(), Some("attackairn"));
        assert!(!register_if_unguessable("attack_air_n", "attackairn"));
    }

    #[test]
    fn a_motion_list_yml_names_motions_and_scripts() {
        let yml = "motion_path: fighter/pacman/motion/body/c00\nlist:\n  attack_s4_s_reverse:\n    game_script: game_attacks4reverse\n    flags:\n      turn: true\n    blend_frames: 0\n    animations:\n      - name: c04attacks4sreverse.nuanmb\n        unk: 0\n    scripts:\n      - expression_attacks4reverse\n      - sound_attacks4reverse\n      - effect_attacks4reverse\n";
        let names = names_from_motion_list_yml(yml);
        assert_eq!(
            names
                .get(&hash40("attack_s4_s_reverse"))
                .map(String::as_str),
            Some("attack_s4_s_reverse")
        );
        assert!(names.contains_key(&hash40("effect_attacks4reverse")));
        assert!(names.contains_key(&hash40("c04attacks4sreverse.nuanmb")));
        // Flag keys are not motions.
        assert!(!names.contains_key(&hash40("turn")));
        let suffix = script_suffix(
            "attack_s4_s_reverse",
            &[hash40("game_attacks4reverse")],
            &names,
        );
        assert_eq!(suffix.as_deref(), Some("attacks4reverse"));
    }

    #[test]
    fn underscores_are_put_back_from_the_motion_hash() {
        let names = vec!["a04walkfallr.nuanmb".to_string()];
        assert_eq!(
            name_from_animation(hash40("walk_fall_r"), &names).as_deref(),
            Some("walk_fall_r")
        );
        let long = vec!["e00specialairnholdloopl.nuanmb".to_string()];
        assert_eq!(
            name_from_animation(hash40("special_air_n_hold_loop_l"), &long).as_deref(),
            Some("special_air_n_hold_loop_l")
        );
        // A wrong hash yields nothing rather than a guess.
        assert_eq!(name_from_animation(hash40("walk_fall_l"), &names), None);
        assert_eq!(
            animation_stem("a04walkfallr.nuanmb").as_deref(),
            Some("walkfallr")
        );
    }

    /// A compiled plugin carries the registration strings even with no source linked.
    #[test]
    fn names_are_recovered_from_a_compiled_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let mut blob = vec![0u8, 0x7f, 0xff];
        blob.extend_from_slice(b"game_attacks4reverse\0attack_s4_s_reverse\0");
        std::fs::write(dir.path().join("plugin.nro"), &blob).unwrap();
        let names = moveset_names(&[], &[dir.path().to_path_buf()]);
        assert_eq!(
            names
                .get(&hash40("attack_s4_s_reverse"))
                .map(String::as_str),
            Some("attack_s4_s_reverse")
        );
        assert!(names.contains_key(&hash40("game_attacks4reverse")));
    }
}
