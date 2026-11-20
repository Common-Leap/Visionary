//! The articles (projectiles and fighter objects) one fighter can spawn, and the resolution of an
//! `ArticleModule::generate_article(..)` id argument to one of them.
//!
//! Resolution is by name only. A generate id's numeric value depends on the build and on clones
//! added by other mods, so it is never consulted.

pub mod clone_compat;
pub mod clones;

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
            let Some(index) = index else { return (Vec::new(), Vec::new()) };
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

        let mut names = BTreeSet::new();
        for sub in ["model", "motion"] {
            for name in subfolders(&fighter_dir.join(sub)) {
                if name != "body" {
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
            });
        }
        for decl in clones.iter().filter(|d| d.new_owner.eq_ignore_ascii_case(&fighter)) {
            let name = decl.new_name.to_lowercase();
            let agent = format!("{fighter}_{name}");
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
            });
        }
        Self { articles, unresolved_clones: unresolved }
    }

    /// The article an id expression names. `locals` maps a local binding to its initializer text.
    pub fn resolve_id(&self, expr: &str, locals: &HashMap<String, String>) -> Option<&Article> {
        self.resolve_depth(expr, locals, 0)
    }

    fn resolve_depth(&self, expr: &str, locals: &HashMap<String, String>, depth: usize) -> Option<&Article> {
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
        if let Some(found) = self.articles.iter().find(|a| a.id_symbols.iter().any(|s| s == ident)) {
            return Some(found);
        }
        let init = locals.get(ident)?;
        self.resolve_depth(init, locals, depth + 1)
    }

    pub fn get(&self, name: &str) -> Option<&Article> {
        self.articles.iter().find(|a| a.name == name || a.agent == name)
    }
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
        assert_eq!(c.articles[0].id_symbols, ["FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL"]);
    }

    #[test]
    fn rewritten_when_source_registers_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(root, "Cargo.toml", "[package]\nname = \"empoleon\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n");
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
        assert_eq!(c.get("blaster_bullet").unwrap().origin, ArticleOrigin::Vanilla);
    }

    #[test]
    fn duplicate_registration_is_a_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(root, "Cargo.toml", "[package]\nname = \"empoleon\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n");
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
        let d = decl("samus", "lativolt", &["FIGHTER_SAMUS_GENERATE_ARTICLE_LATIVOLT"], None);
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
        assert_eq!(c.articles[0].id_note.as_deref(), Some("stored in a struct field"));
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
        let d = decl("samus", "lativolt", &["FIGHTER_SAMUS_GENERATE_ARTICLE_LATIVOLT"], None);
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
            assert_eq!(c.resolve_id(e, &none).map(|a| a.name.as_str()), Some("fireball"), "{e}");
        }
    }

    #[test]
    fn resolves_local() {
        let c = named("pacman", "trampoline");
        let locals = HashMap::from([(
            "generate_id".to_string(),
            "*FIGHTER_PACMAN_GENERATE_ARTICLE_TRAMPOLINE".to_string(),
        )]);
        assert_eq!(c.resolve_id("generate_id", &locals).unwrap().name, "trampoline");
    }

    #[test]
    fn unknown_is_none() {
        let c = named("mario", "fireball");
        assert!(c.resolve_id("owner_article(x)", &HashMap::new()).is_none());
        assert!(c.resolve_id("some_local", &HashMap::new()).is_none());
    }
}
