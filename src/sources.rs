//! Checks that a folder picked as the ArcExplorer dump really is one.
//!
//! A mod folder picked by mistake looks plausible (it has `fighter/` and
//! `effect/`) but lacks most of the game, so every later lookup would quietly
//! come up empty. [`check_dump`] tells the two apart so the UI can warn.

use std::path::{Path, PathBuf};

use crate::eff_editor::CARRIER_BASE_REL;

/// A full dump has about 90 fighter folders; fewer than this smells like a mod.
pub const FEW_FIGHTERS: usize = 20;

/// How deep (counted from the picked folder) to look for plugin `.nro` files.
const NRO_DEPTH: usize = 3;

/// One reason a folder looks like a mod rather than a game dump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModHint {
    InfoToml,
    Plugin,
    RomfsWrapper,
    FewFighters(usize),
    NotVanilla,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DumpCheck {
    Ok,
    /// `fighter/` and/or `effect/` isn't a folder here. `suggestion` is a
    /// nearby folder that does hold both, when there is one.
    Missing {
        fighter: bool,
        effect: bool,
        suggestion: Option<PathBuf>,
    },
    /// Has both folders, but looks like a mod. Hints are in declaration order.
    LooksLikeMod(Vec<ModHint>),
}

fn has_both(dir: &Path) -> bool {
    dir.join("fighter").is_dir() && dir.join("effect").is_dir()
}

/// The folder the user probably meant: the parent (they picked `fighter/`
/// itself) or the single child that holds both folders (they picked the folder
/// above the dump).
fn suggest(dir: &Path) -> Option<PathBuf> {
    if let Some(parent) = dir.parent() {
        if !parent.as_os_str().is_empty() && has_both(parent) {
            return Some(parent.to_path_buf());
        }
    }
    let mut found = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && has_both(p));
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

/// Whether a `.nro` exists within `NRO_DEPTH` of `dir`. Skips symlinks and
/// never enters `fighter/` or `effect/`, which are huge in a real dump.
fn has_nro(dir: &Path, depth: usize) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let path = entry.path();
        if ft.is_file() {
            let is_nro = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("nro"));
            if is_nro {
                return true;
            }
        } else if ft.is_dir() && depth + 1 < NRO_DEPTH {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if depth == 0 && (name == "fighter" || name == "effect") {
                continue;
            }
            if has_nro(&path, depth + 1) {
                return true;
            }
        }
    }
    false
}

fn has_romfs_wrapper(dir: &Path) -> bool {
    let own = dir
        .file_name()
        .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("romfs"));
    own || std::fs::read_dir(dir).is_ok_and(|rd| {
        rd.flatten().any(|e| {
            e.path().is_dir() && e.file_name().to_string_lossy().eq_ignore_ascii_case("romfs")
        })
    })
}

pub fn check_dump(dir: &Path) -> DumpCheck {
    let fighter = dir.join("fighter").is_dir();
    let effect = dir.join("effect").is_dir();
    if !fighter || !effect {
        return DumpCheck::Missing {
            fighter: !fighter,
            effect: !effect,
            suggestion: suggest(dir),
        };
    }
    let mut hints = Vec::new();
    if dir.join("info.toml").exists() {
        hints.push(ModHint::InfoToml);
    }
    if has_nro(dir, 0) {
        hints.push(ModHint::Plugin);
    }
    if has_romfs_wrapper(dir) {
        hints.push(ModHint::RomfsWrapper);
    }
    let fighters = std::fs::read_dir(dir.join("fighter"))
        .map(|rd| rd.flatten().filter(|e| e.path().is_dir()).count())
        .unwrap_or(0);
    if fighters < FEW_FIGHTERS {
        hints.push(ModHint::FewFighters(fighters));
    }
    if !dir.join(CARRIER_BASE_REL).is_file() {
        hints.push(ModHint::NotVanilla);
    }
    if hints.is_empty() {
        DumpCheck::Ok
    } else {
        DumpCheck::LooksLikeMod(hints)
    }
}

/// A plain-language reason for a hint, for the warning shown to the modder.
pub fn hint_text(hint: &ModHint) -> String {
    match hint {
        ModHint::InfoToml => "has an info.toml, which mods ship and dumps don't".into(),
        ModHint::Plugin => "contains a plugin (.nro file), which a game dump never does".into(),
        ModHint::RomfsWrapper => {
            "is, or contains, a romfs folder — pick the folder that contains fighter/ and effect/"
                .into()
        }
        ModHint::FewFighters(n) => {
            format!("holds only {n} fighters — a full dump has about 90")
        }
        ModHint::NotVanilla => {
            format!(
                "is missing {CARRIER_BASE_REL}, a file every full dump has, so it won't be used to fill in files a partial romfs copy leaves out"
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    fn make_vanilla(root: &Path) {
        for i in 0..FEW_FIGHTERS {
            fs::create_dir_all(root.join(format!("fighter/f{i}"))).unwrap();
        }
        touch(&root.join(CARRIER_BASE_REL));
    }

    #[test]
    fn vanilla_like_dump_is_ok() {
        let d = tempfile::tempdir().unwrap();
        make_vanilla(d.path());
        assert_eq!(check_dump(d.path()), DumpCheck::Ok);
    }

    #[test]
    fn mod_folder_flags_hints() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("fighter/mario")).unwrap();
        fs::create_dir_all(d.path().join("effect")).unwrap();
        touch(&d.path().join("info.toml"));
        touch(&d.path().join("plugin.nro"));
        let DumpCheck::LooksLikeMod(h) = check_dump(d.path()) else {
            panic!("expected LooksLikeMod");
        };
        assert_eq!(
            h,
            vec![
                ModHint::InfoToml,
                ModHint::Plugin,
                ModHint::FewFighters(1),
                ModHint::NotVanilla
            ]
        );
    }

    #[test]
    fn suggests_parent_when_fighter_folder_picked() {
        let d = tempfile::tempdir().unwrap();
        make_vanilla(d.path());
        let picked = d.path().join("fighter");
        assert_eq!(
            check_dump(&picked),
            DumpCheck::Missing {
                fighter: true,
                effect: true,
                suggestion: Some(d.path().to_path_buf())
            }
        );
    }

    #[test]
    fn suggests_child_dump() {
        let p = tempfile::tempdir().unwrap();
        let child = p.path().join("export");
        make_vanilla(&child);
        match check_dump(p.path()) {
            DumpCheck::Missing { suggestion, .. } => assert_eq!(suggestion, Some(child)),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn empty_folder_is_missing_without_suggestion() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            check_dump(d.path()),
            DumpCheck::Missing {
                fighter: true,
                effect: true,
                suggestion: None
            }
        );
    }

    #[test]
    fn romfs_wrapper_detected() {
        let d = tempfile::tempdir().unwrap();
        make_vanilla(d.path());
        fs::create_dir_all(d.path().join("romfs")).unwrap();
        assert_eq!(
            check_dump(d.path()),
            DumpCheck::LooksLikeMod(vec![ModHint::RomfsWrapper])
        );
    }
}
