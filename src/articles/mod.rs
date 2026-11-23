//! The articles (projectiles and fighter objects) one fighter can spawn, and the resolution of an
//! `ArticleModule::generate_article(..)` id argument to one of them.
//!
//! Resolution is by name only. A generate id's numeric value depends on the build and on clones
//! added by other mods, so it is never consulted.

pub mod clone_compat;
pub mod clones;
#[cfg(test)]
mod corpus_sweep;
pub mod params;
pub mod settings;
pub mod view;

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use crate::acmd_src::SourceIndex;
use clones::{CloneDecl, UnresolvedClone};

#[derive(Debug, Clone, PartialEq)]
pub enum ArticleOrigin {
    /// Ships with the game (a folder under the fighter's `model` or `motion`).
    Vanilla,
    /// A vanilla article whose scripts the moveset's source registers.
    Rewritten,
    /// Added by the moveset through `smashline::clone_weapon`.
    Clone(CloneDecl),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Article {
    pub owner: String,
    pub name: String,
    /// `owner_name`, the smashline agent that scripts for it register under.
    pub agent: String,
    pub origin: ArticleOrigin,
    /// Names the generate id can be written as: `FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL` for a
    /// vanilla article, the clone's `id_bindings` for a clone.
    pub id_symbols: Vec<String>,
    /// Script names the moveset's source registers for this article.
    pub source_scripts: Vec<String>,
    /// Scripts registered in more than one place, which the index leaves unselected.
    pub conflicts: Vec<String>,
    /// For a clone: why its id is not bound to a name Visionary follows. `None` otherwise.
    pub id_note: Option<String>,
    /// For a clone: the other `clone_weapon` calls (file, line) that declare this same article,
    /// such as a vendored copy of the crate. The first call is the one in `origin`.
    pub also_declared: Vec<(std::path::PathBuf, usize)>,
    /// For a clone declared more than once from different sources: one sentence per other
    /// declaration, naming both sources and where each is. The first declaration is `origin`.
    pub clone_conflicts: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ArticleCatalog {
    pub articles: Vec<Article>,
    pub unresolved_clones: Vec<UnresolvedClone>,
}

fn subfolders(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().to_lowercase())
        .collect()
}

impl ArticleCatalog {
    pub fn build(
        fighter: &str,
        index: Option<&SourceIndex>,
        clones: &[CloneDecl],
        unresolved: Vec<UnresolvedClone>,
        fighter_dir: &Path,
    ) -> Self {
        let fighter = fighter.to_lowercase();
        // (every script the source registers for the agent, those registered twice)
        let scripts_of = |agent: &str| -> (Vec<String>, Vec<String>) {
            let Some(index) = index else {
                return (Vec::new(), Vec::new());
            };
            let mut scripts: Vec<String> = index
                .fighters
                .get(agent)
                .map(|f| f.scripts.keys().cloned().collect())
                .unwrap_or_default();
            let mut conflicts: Vec<String> = index
                .conflicts
                .iter()
                .filter(|c| c.fighter == agent)
                .map(|c| c.script.clone())
                .collect();
            scripts.extend(conflicts.iter().cloned());
            scripts.sort();
            scripts.dedup();
            conflicts.sort();
            conflicts.dedup();
            (scripts, conflicts)
        };

        let clones: Vec<&CloneDecl> = clones
            .iter()
            .filter(|d| d.new_owner.eq_ignore_ascii_case(&fighter))
            .collect();
        // A folder a clone of this fighter adds is that clone's, not a vanilla article: the
        // moveset ships the clone's model under its new owner (Latios's `samus/model/lativolt`).
        let cloned: BTreeSet<String> = clones.iter().map(|d| d.new_name.to_lowercase()).collect();
        let mut names = BTreeSet::new();
        for sub in ["model", "motion"] {
            for name in subfolders(&fighter_dir.join(sub)) {
                if name != "body" && !cloned.contains(&name) {
                    names.insert(name);
                }
            }
        }

        let mut articles = Vec::new();
        for name in names {
            let agent = format!("{fighter}_{name}");
            let (source_scripts, conflicts) = scripts_of(&agent);
            articles.push(Article {
                id_symbols: vec![format!(
                    "FIGHTER_{}_GENERATE_ARTICLE_{}",
                    fighter.to_uppercase(),
                    name.to_uppercase()
                )],
                origin: if source_scripts.is_empty() {
                    ArticleOrigin::Vanilla
                } else {
                    ArticleOrigin::Rewritten
                },
                owner: fighter.clone(),
                name,
                agent,
                source_scripts,
                conflicts,
                id_note: None,
                also_declared: Vec::new(),
                clone_conflicts: Vec::new(),
            });
        }
        for decl in clones {
            let name = decl.new_name.to_lowercase();
            let agent = format!("{fighter}_{name}");
            // The same article declared again (HDR's vendored crate beside its port) is one
            // article; the other calls are kept on it. Declared again from a different source,
            // it is still one object in game, but which source wins is not known: say so.
            if let Some(first) = articles.iter_mut().find(|a| a.agent == agent) {
                let ArticleOrigin::Clone(kept) = &mut first.origin else {
                    first.also_declared.push((decl.file.clone(), decl.line));
                    continue;
                };
                let source = |d: &CloneDecl| {
                    format!(
                        "{}/{}{}",
                        d.src_owner.as_deref().unwrap_or("?"),
                        d.src_kind,
                        if d.use_original_code {
                            " with its original code"
                        } else {
                            ""
                        }
                    )
                };
                if (&kept.src_owner, &kept.src_kind, kept.use_original_code)
                    != (&decl.src_owner, &decl.src_kind, decl.use_original_code)
                {
                    first.clone_conflicts.push(format!(
                        "Declared as a clone of {} at {}:{} and of {} at {}:{}; Visionary shows the first",
                        source(kept),
                        kept.file.display(),
                        kept.line,
                        source(decl),
                        decl.file.display(),
                        decl.line
                    ));
                    continue;
                }
                first.also_declared.push((decl.file.clone(), decl.line));
                for binding in &decl.id_bindings {
                    if !first.id_symbols.contains(binding) {
                        first.id_symbols.push(binding.clone());
                    }
                    if !kept.id_bindings.contains(binding) {
                        kept.id_bindings.push(binding.clone());
                    }
                }
                for binding in &decl.kind_bindings {
                    if !kept.kind_bindings.contains(binding) {
                        kept.kind_bindings.push(binding.clone());
                    }
                }
                if !first.id_symbols.is_empty() {
                    first.id_note = None;
                    kept.id_note = None;
                }
                continue;
            }
            let (source_scripts, conflicts) = scripts_of(&agent);
            articles.push(Article {
                owner: fighter.clone(),
                name,
                agent,
                origin: ArticleOrigin::Clone(decl.clone()),
                id_symbols: decl.id_bindings.clone(),
                source_scripts,
                conflicts,
                id_note: decl.id_note.clone(),
                also_declared: Vec::new(),
                clone_conflicts: Vec::new(),
            });
        }
        Self {
            articles,
            unresolved_clones: unresolved,
        }
    }

    /// The article an id expression names. `locals` maps a local binding to its initializer text.
    pub fn resolve_id(&self, expr: &str, locals: &HashMap<String, String>) -> Option<&Article> {
        self.resolve_depth(expr, locals, 0)
    }

    fn resolve_depth(
        &self,
        expr: &str,
        locals: &HashMap<String, String>,
        depth: usize,
    ) -> Option<&Article> {
        if depth > 8 {
            return None;
        }
        let mut text = expr.trim();
        loop {
            let before = text;
            text = text.trim_start_matches(['*', '&']).trim();
            if let Some(rest) = text.strip_suffix(" as i32") {
                text = rest.trim_end();
            }
            if text.len() > 1 && text.starts_with('(') && text.ends_with(')') {
                text = text[1..text.len() - 1].trim();
                continue;
            }
            for cut in [".get(", ".load("] {
                if let Some(at) = text.find(cut) {
                    text = text[..at].trim_end();
                }
            }
            if text == before {
                break;
            }
        }
        let ident = text.rsplit("::").next().unwrap_or(text);
        if ident.is_empty() || !ident.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return None;
        }
        if let Some(found) = self
            .articles
            .iter()
            .find(|a| a.id_symbols.iter().any(|s| s == ident))
        {
            return Some(found);
        }
        if let Some(found) = self.by_vanilla_spelling(ident) {
            return Some(found);
        }
        let init = locals.get(ident)?;
        self.resolve_depth(init, locals, depth + 1)
    }

    /// A vanilla id the game spells differently from the article's folder:
    /// `FIGHTER_MARIO_GENERATE_ARTICLE_HUGE_FLAME` for `mario/model/hugeflame`, and the id an
    /// article uses for its owner's other articles, `WEAPON_MURABITO_SLINGSHOT_GENERATE_ARTICLE_BULLET`
    /// for `murabito/model/bullet`, and `FIGHTER_PEACH_GENERATE_ARTICLE_DAIKON` for
    /// `peach/motion/peachdaikon`. Only the game's own articles (clones are bound by name), and
    /// only when exactly one matches.
    fn by_vanilla_spelling(&self, ident: &str) -> Option<&Article> {
        let (head, name) = ident.split_once("_GENERATE_ARTICLE_")?;
        let squash = |s: &str| s.replace('_', "").to_lowercase();
        let name = squash(name);
        let mut found = self.articles.iter().filter(|a| {
            let owner = a.owner.to_uppercase();
            let owned = head == format!("FIGHTER_{owner}")
                || head
                    .strip_prefix(&format!("WEAPON_{owner}_"))
                    .is_some_and(|rest| !rest.is_empty());
            // The folder may also carry the owner's name: `peach/motion/peachdaikon`.
            let folder = squash(&a.name);
            owned
                && !matches!(a.origin, ArticleOrigin::Clone(_))
                && (folder == name || folder == format!("{}{name}", a.owner.to_lowercase()))
        });
        let first = found.next()?;
        found.next().is_none().then_some(first)
    }

    pub fn get(&self, name: &str) -> Option<&Article> {
        self.articles
            .iter()
            .find(|a| a.name == name || a.agent == name)
    }
}

/// The project's Rust files with their text, as the index read them.
fn rust_sources(index: &SourceIndex) -> Vec<(std::path::PathBuf, String)> {
    index
        .files
        .iter()
        .filter(|file| file.extension().is_some_and(|e| e == "rs"))
        .filter_map(|file| Some((file.clone(), std::fs::read_to_string(file).ok()?)))
        .collect()
}

/// `fighter`'s objects as the linked moveset sees them: the dump's (from `fighter_dir`), the ones
/// the source rewrites, and the clones it adds. `fighter_names` are every fighter folder name, to
/// tell a clone's source owner from its kind.
pub fn catalog_for(
    fighter: &str,
    index: &SourceIndex,
    fighter_names: &[String],
    fighter_dir: &Path,
) -> ArticleCatalog {
    // Only a file that registers a clone or defines a const can matter to the clone reader (the
    // consts name owners and kinds); the rest are not parsed.
    let sources: Vec<(std::path::PathBuf, String)> = rust_sources(index)
        .into_iter()
        .filter(|(_, text)| text.contains("clone_weapon") || text.contains("const "))
        .collect();
    let (clones, unresolved) = clones::read_clones(&sources, &index.consts, fighter_names);
    ArticleCatalog::build(fighter, Some(index), &clones, unresolved, fighter_dir)
}

/// The moveset's `param_config` overrides of params in `fighter`'s own file (its weapons'
/// blocks included), and every call in the project that could not be attributed to a fighter.
/// `fighter_dirs` are the fighter folder names, to name a weapon kind's owner.
pub fn param_overrides_for(
    index: &SourceIndex,
    fighter: &str,
    fighter_dirs: &[String],
) -> (Vec<params::ParamOverride>, Vec<params::UnreadOverride>) {
    let sources: Vec<(std::path::PathBuf, String)> = rust_sources(index)
        .into_iter()
        .filter(|(_, text)| text.contains("param_config"))
        .collect();
    let (all, unread) = params::param_config_overrides(&sources, fighter_dirs);
    (
        all.into_iter()
            .filter(|o| o.is_for_fighter(fighter))
            .collect(),
        unread,
    )
}

/// The settings an article's status scripts write, following the project functions they call
/// (looked up in the caller's file first, then anywhere in the project when only one has it).
pub fn settings_for(index: &SourceIndex, agent: &str) -> Vec<settings::Setting> {
    let bodies = index.status_bodies(agent);
    if bodies.is_empty() {
        return Vec::new();
    }
    let sources = rust_sources(index);
    let helpers = |caller: &Path,
                   name: &str|
     -> Option<(std::path::PathBuf, std::ops::Range<usize>, String)> {
        let in_file = |(file, text): &(std::path::PathBuf, String)| {
            crate::acmd_src::function_item(text, name)
                .map(|span| (file.clone(), span.clone(), text[span].to_string()))
        };
        if let Some(found) = sources
            .iter()
            .filter(|(file, _)| file == caller)
            .find_map(in_file)
        {
            return Some(found);
        }
        let mut all = sources.iter().filter_map(in_file);
        let first = all.next()?;
        all.next().is_none().then_some(first)
    };
    settings::read_settings(&bodies, &index.consts, &helpers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn put(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn decl(owner: &str, name: &str, bindings: &[&str], note: Option<&str>) -> CloneDecl {
        CloneDecl {
            new_owner: owner.into(),
            new_name: name.into(),
            src_owner: Some("mario".into()),
            src_kind: "WEAPON_KIND_MARIO_FIREBALL".into(),
            use_original_code: false,
            id_bindings: bindings.iter().map(|s| s.to_string()).collect(),
            kind_bindings: vec![],
            id_note: note.map(String::from),
            file: PathBuf::from("lib.rs"),
            line: 1,
        }
    }

    fn named(fighter: &str, article: &str) -> ArticleCatalog {
        let tmp = tempfile::tempdir().unwrap();
        put(tmp.path(), &format!("model/{article}/c00/a.txt"), "");
        ArticleCatalog::build(fighter, None, &[], vec![], tmp.path())
    }

    const FLY: &str = r#"unsafe extern "C" fn game_fly(agent: &mut L2CAgentBase) {}
pub fn install() {
    Agent::new("falco_cannon").acmd("game_fly", game_fly, Priority::Default).install();
}
"#;

    #[test]
    fn vanilla_from_folders() {
        let tmp = tempfile::tempdir().unwrap();
        put(tmp.path(), "model/fireball/c00/a.txt", "");
        put(tmp.path(), "model/body/c00/a.txt", "");
        let c = ArticleCatalog::build("mario", None, &[], vec![], tmp.path());
        assert_eq!(c.articles.len(), 1);
        assert_eq!(c.articles[0].name, "fireball");
        assert_eq!(c.articles[0].origin, ArticleOrigin::Vanilla);
        assert_eq!(
            c.articles[0].id_symbols,
            ["FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL"]
        );
    }

    #[test]
    fn rewritten_when_source_registers_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(
            root,
            "Cargo.toml",
            "[package]\nname = \"empoleon\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n",
        );
        put(root, "src/lib.rs", FLY);
        let index = SourceIndex::build(root).unwrap();
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "model/cannon/c00/a.txt", "");
        put(dir.path(), "model/blaster_bullet/c00/a.txt", "");
        let c = ArticleCatalog::build("falco", Some(&index), &[], vec![], dir.path());
        let cannon = c.get("cannon").unwrap();
        assert_eq!(cannon.origin, ArticleOrigin::Rewritten);
        assert_eq!(cannon.agent, "falco_cannon");
        assert_eq!(cannon.source_scripts, ["game_fly"]);
        assert_eq!(
            c.get("blaster_bullet").unwrap().origin,
            ArticleOrigin::Vanilla
        );
    }

    #[test]
    fn duplicate_registration_is_a_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(
            root,
            "Cargo.toml",
            "[package]\nname = \"empoleon\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n",
        );
        put(root, "src/lib.rs", "mod a;\nmod b;\n");
        put(root, "src/a.rs", FLY);
        put(root, "src/b.rs", FLY);
        let index = SourceIndex::build(root).unwrap();
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "model/cannon/c00/a.txt", "");
        let c = ArticleCatalog::build("falco", Some(&index), &[], vec![], dir.path());
        let cannon = c.get("cannon").unwrap();
        assert_eq!(cannon.conflicts, ["game_fly"]);
        assert_eq!(cannon.origin, ArticleOrigin::Rewritten);
    }

    #[test]
    fn clone_listed_for_new_owner() {
        let d = decl(
            "samus",
            "lativolt",
            &["FIGHTER_SAMUS_GENERATE_ARTICLE_LATIVOLT"],
            None,
        );
        let other = decl("mario", "thing", &["T"], None);
        let tmp = tempfile::tempdir().unwrap();
        let c = ArticleCatalog::build("samus", None, &[d.clone(), other], vec![], tmp.path());
        assert_eq!(c.articles.len(), 1);
        assert_eq!(c.articles[0].agent, "samus_lativolt");
        assert_eq!(c.articles[0].origin, ArticleOrigin::Clone(d));
        assert_eq!(c.articles[0].id_note, None);

        let unbound = decl("samus", "lativolt", &[], Some("stored in a struct field"));
        let c = ArticleCatalog::build("samus", None, &[unbound], vec![], tmp.path());
        assert_eq!(c.articles.len(), 1, "an unbound clone is still listed");
        assert_eq!(
            c.articles[0].id_note.as_deref(),
            Some("stored in a struct field")
        );
    }

    #[test]
    fn resolves_deref_const() {
        let c = named("mario", "fireball");
        let a = c.resolve_id("*FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL", &HashMap::new());
        assert_eq!(a.unwrap().name, "fireball");
    }

    #[test]
    fn resolves_plain_static() {
        let tmp = tempfile::tempdir().unwrap();
        let d = decl(
            "samus",
            "lativolt",
            &["FIGHTER_SAMUS_GENERATE_ARTICLE_LATIVOLT"],
            None,
        );
        let c = ArticleCatalog::build("samus", None, &[d], vec![], tmp.path());
        let a = c.resolve_id("FIGHTER_SAMUS_GENERATE_ARTICLE_LATIVOLT", &HashMap::new());
        assert_eq!(a.unwrap().name, "lativolt");
    }

    #[test]
    fn resolves_once_lock_get() {
        let c = named("kingteresa", "fireball");
        let none = HashMap::new();
        for e in [
            "*FIGHTER_KINGTERESA_GENERATE_ARTICLE_FIREBALL.get().unwrap()",
            "(*FIGHTER_KINGTERESA_GENERATE_ARTICLE_FIREBALL.get().copied().unwrap_or(0)) as i32",
            "FIGHTER_KINGTERESA_GENERATE_ARTICLE_FIREBALL.load(Ordering::Relaxed)",
        ] {
            assert_eq!(
                c.resolve_id(e, &none).map(|a| a.name.as_str()),
                Some("fireball"),
                "{e}"
            );
        }
    }

    #[test]
    fn resolves_local() {
        let c = named("pacman", "trampoline");
        let locals = HashMap::from([(
            "generate_id".to_string(),
            "*FIGHTER_PACMAN_GENERATE_ARTICLE_TRAMPOLINE".to_string(),
        )]);
        assert_eq!(
            c.resolve_id("generate_id", &locals).unwrap().name,
            "trampoline"
        );
    }

    /// Latios-Moveset · src/lib.rs:559 clones `lativolt` onto samus and ships its model as
    /// `romfs/fighter/samus/model/lativolt` (tier 1, smashline2). The folder is the clone's,
    /// so the object is listed once, as the clone.
    #[test]
    fn a_clones_own_folder_is_not_a_vanilla_article() {
        let tmp = tempfile::tempdir().unwrap();
        put(tmp.path(), "model/lativolt/c00/a.txt", "");
        put(tmp.path(), "model/cshot/c00/a.txt", "");
        let d = decl(
            "samus",
            "lativolt",
            &["FIGHTER_SAMUS_GENERATE_ARTICLE_LATIVOLT"],
            None,
        );
        let c = ArticleCatalog::build("samus", None, &[d.clone()], vec![], tmp.path());
        let agents: Vec<&str> = c.articles.iter().map(|a| a.agent.as_str()).collect();
        assert_eq!(agents, ["samus_cshot", "samus_lativolt"]);
        assert_eq!(c.get("lativolt").unwrap().origin, ArticleOrigin::Clone(d));
    }

    /// HDR-Standalone-Movesets · ports/fighters/packun/src/lib.rs:102 and
    /// vendor/hdr/fighters/packun/src/lib.rs:101 both call
    /// `clone_weapon("mario", *WEAPON_KIND_MARIO_FIREBALL, "packun", "firebreath", false)`
    /// (tier 1, smashline2). One object, with the second call kept on it.
    #[test]
    fn the_same_clone_declared_twice_is_one_article() {
        let tmp = tempfile::tempdir().unwrap();
        let a = decl("packun", "firebreath", &[], None);
        let b = CloneDecl {
            file: PathBuf::from("vendor/lib.rs"),
            line: 101,
            ..a.clone()
        };
        let c = ArticleCatalog::build("packun", None, &[a.clone(), b], vec![], tmp.path());
        assert_eq!(c.articles.len(), 1);
        assert_eq!(c.articles[0].origin, ArticleOrigin::Clone(a));
        assert_eq!(
            c.articles[0].also_declared,
            [(PathBuf::from("vendor/lib.rs"), 101)]
        );
    }

    /// R20: two declarations of one clone from different sources are a conflict, kept with both;
    /// from the same source, their bindings merge.
    #[test]
    fn a_clone_declared_from_two_sources_is_a_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let a = decl("packun", "firebreath", &[], Some("unbound"));
        let b = CloneDecl {
            src_kind: "WEAPON_KIND_KOOPA_BREATH".into(),
            src_owner: Some("koopa".into()),
            file: PathBuf::from("vendor/lib.rs"),
            line: 101,
            ..a.clone()
        };
        let c = ArticleCatalog::build("packun", None, &[a.clone(), b], vec![], tmp.path());
        assert_eq!(c.articles.len(), 1);
        let article = &c.articles[0];
        assert_eq!(article.origin, ArticleOrigin::Clone(a.clone()));
        assert_eq!(article.clone_conflicts.len(), 1);
        assert!(
            article.also_declared.is_empty(),
            "conflict should not duplicate into also_declared"
        );
        let note = &article.clone_conflicts[0];
        assert!(
            note.contains("mario/WEAPON_KIND_MARIO_FIREBALL")
                && note.contains("koopa/WEAPON_KIND_KOOPA_BREATH"),
            "{note}"
        );
        assert!(
            note.contains("lib.rs:1") && note.contains("vendor/lib.rs:101"),
            "{note}"
        );
        assert!(crate::articles::view::clone_notes(article)
            .iter()
            .any(|n| n == note));

        let same = CloneDecl {
            id_bindings: vec!["FIGHTER_PACKUN_GENERATE_ARTICLE_FIREBREATH".into()],
            kind_bindings: vec!["WEAPON_KIND_PACKUN_FIREBREATH".into()],
            id_note: None,
            file: PathBuf::from("vendor/lib.rs"),
            line: 101,
            ..a.clone()
        };
        let c = ArticleCatalog::build("packun", None, &[a, same], vec![], tmp.path());
        let article = &c.articles[0];
        assert!(article.clone_conflicts.is_empty());
        assert_eq!(
            article.also_declared,
            [(PathBuf::from("vendor/lib.rs"), 101)]
        );
        assert_eq!(
            article.id_symbols,
            ["FIGHTER_PACKUN_GENERATE_ARTICLE_FIREBREATH"]
        );
        assert_eq!(article.id_note, None);
        let ArticleOrigin::Clone(kept) = &article.origin else {
            panic!()
        };
        assert_eq!(kept.kind_bindings, ["WEAPON_KIND_PACKUN_FIREBREATH"]);
        assert_eq!(
            kept.id_bindings,
            ["FIGHTER_PACKUN_GENERATE_ARTICLE_FIREBREATH"]
        );
    }

    /// Iron-Man · src/stark/status/final_proton.rs:64 spawns
    /// `*FIGHTER_MARIO_GENERATE_ARTICLE_HUGE_FLAME` (0x3 in the game's lua consts) for the dump's
    /// `mario/model/hugeflame`; Toad-Moveset · plugin/src/toad/acmd/ground.rs:231 spawns
    /// `*WEAPON_MURABITO_SLINGSHOT_GENERATE_ARTICLE_BULLET` (0x0) for `murabito/model/bullet`
    /// (both tier 1, smashline2).
    #[test]
    fn resolves_the_games_own_spellings() {
        let c = named("mario", "hugeflame");
        let none = HashMap::new();
        assert_eq!(
            c.resolve_id("*FIGHTER_MARIO_GENERATE_ARTICLE_HUGE_FLAME", &none)
                .unwrap()
                .name,
            "hugeflame"
        );
        let c = named("murabito", "bullet");
        assert_eq!(
            c.resolve_id("*WEAPON_MURABITO_SLINGSHOT_GENERATE_ARTICLE_BULLET", &none)
                .unwrap()
                .name,
            "bullet"
        );
        // HDR-Standalone-Movesets · vendor/hdr/fighters/peach/src/acmd/specials.rs:379 spawns
        // `*FIGHTER_PEACH_GENERATE_ARTICLE_DAIKON` (0x3); the dump's folder is `peachdaikon`.
        let c = named("peach", "peachdaikon");
        assert_eq!(
            c.resolve_id("*FIGHTER_PEACH_GENERATE_ARTICLE_DAIKON", &none)
                .unwrap()
                .name,
            "peachdaikon"
        );
        let c = named("murabito", "bullet");
        // Another fighter's id is not this fighter's article.
        assert!(c
            .resolve_id("*FIGHTER_SHIZUE_GENERATE_ARTICLE_BULLET", &none)
            .is_none());
        assert!(c
            .resolve_id("*WEAPON_SHIZUE_SLINGSHOT_GENERATE_ARTICLE_BULLET", &none)
            .is_none());
    }

    #[test]
    fn unknown_is_none() {
        let c = named("mario", "fireball");
        assert!(c.resolve_id("owner_article(x)", &HashMap::new()).is_none());
        assert!(c.resolve_id("some_local", &HashMap::new()).is_none());
    }
}
