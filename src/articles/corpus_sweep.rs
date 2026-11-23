//! The article corpus sweep: every real moveset under `VISIONARY_MOVESETS`, read the way the app
//! reads a linked moveset, with a report of every article, every id, every setting and a byte
//! round trip of every script that has article calls.
//!
//! Ignored by default. Run it with
//!
//! ```text
//! VISIONARY_MOVESETS="…/Movesets" cargo test --bin visionary corpus_sweep -- --ignored --nocapture
//! ```
//!
//! `VISIONARY_DUMP` names the ArcExplorer dump (the folder holding `fighter/`); unset, the data
//! root saved in Visionary's config is used. The sweep only reads the movesets and the dump: the
//! round trips are done on text in memory, never written anywhere. The report is written to
//! `target/article-corpus.md`.
//!
//! The test fails on a round trip that changes a byte it should not, on an article listed under
//! two owners, and on an id that resolves to another owner's article. Everything else it finds
//! (ids, clones, statuses it cannot follow) is residue, listed with file and line.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::settings::SettingValue;
use super::{ArticleCatalog, ArticleOrigin};
use crate::acmd_src::{ScriptSite, SourceIndex};
use crate::data::{ArticleCall, ArticleOp};

/// Folder names never swept, as backups or tool scratch: the index skips dot folders itself, these
/// are the top-level ones that are copies of another moveset.
fn skip_reason(name: &str) -> Option<&'static str> {
    if name.contains("(OLD)") {
        return Some("an older copy of a moveset that is swept under its own name");
    }
    if name.ends_with("-stash") {
        return Some("a stash copy of part of another moveset");
    }
    None
}

/// The folder a moveset's source is linked at: the folder itself when it holds a `Cargo.toml`,
/// else the shallowest folder under it that does (outside dot folders and `target`).
fn crate_root(dir: &Path) -> Option<PathBuf> {
    let mut level = vec![dir.to_path_buf()];
    for _ in 0..3 {
        let mut next = Vec::new();
        let mut found: Vec<PathBuf> = Vec::new();
        for d in &level {
            if d.join("Cargo.toml").is_file() {
                found.push(d.clone());
                continue;
            }
            for entry in std::fs::read_dir(d).into_iter().flatten().flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if entry.path().is_dir() && !name.starts_with('.') && name != "target" {
                    next.push(entry.path());
                }
            }
        }
        if !found.is_empty() {
            found.sort();
            return found.into_iter().next();
        }
        level = next;
    }
    None
}

fn dump_root() -> Option<PathBuf> {
    if let Some(dump) = std::env::var_os("VISIONARY_DUMP") {
        return Some(PathBuf::from(dump));
    }
    // R4: the data root Visionary saved, read without `load_config_path`'s legacy migration so
    // the sweep writes nothing.
    let saved = dirs::config_dir()?.join("visionary").join("data_root");
    let path = PathBuf::from(std::fs::read_to_string(saved).ok()?.trim());
    path.is_dir().then_some(path)
}

fn subdirs(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_lowercase())
        .collect();
    out.sort();
    out
}

fn line_of(text: &str, at: usize) -> usize {
    text.as_bytes()[..at.min(text.len())]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

/// `agent` split into its owner and its article name, by the longest fighter name it starts with.
/// `None` for an agent that names no fighter.
pub(super) fn split_agent(agent: &str, names: &[String]) -> Option<(String, Option<String>)> {
    if names.iter().any(|n| n == agent) {
        return Some((agent.to_string(), None));
    }
    names
        .iter()
        .filter(|n| {
            agent.len() > n.len() + 1
                && agent.starts_with(n.as_str())
                && agent.as_bytes()[n.len()] == b'_'
        })
        .max_by_key(|n| n.len())
        .map(|n| (n.clone(), Some(agent[n.len() + 1..].to_string())))
}

/// Fighters whose scripts write another fighter's article ids on purpose, and whose: Kirby's copy
/// abilities spawn the copied fighter's articles, and an echo or semi-clone shares its original's
/// list. Such an id is residue to list, never a misattribution.
const CROSS_FIGHTER: &[(&str, &str)] = &[
    ("samusd", "samus"),
    ("pichu", "pikachu"),
    ("richter", "simon"),
    ("falco", "fox"),
    ("shizue", "murabito"),
    ("daisy", "peach"),
    ("lucina", "marth"),
    ("chrom", "roy"),
    ("ken", "ryu"),
    ("pitb", "pit"),
];

pub(super) fn cross_fighter_ok(owner: &str, other: &str) -> bool {
    owner == "kirby" || CROSS_FIGHTER.contains(&(owner, other))
}

/// The fighter an id symbol names when it is a fighter other than `owner`:
/// `FIGHTER_<X>_GENERATE_ARTICLE_*`, or `WEAPON_<X>_<ARTICLE>_GENERATE_ARTICLE_*` for an article's
/// own. A name that is no fighter (`FIGHTER_KINGTERESA_…`, a moveset's own) names nobody else.
pub(super) fn foreign_id_owner(symbol: &str, owner: &str, names: &[String]) -> Option<String> {
    let ident = symbol.trim().trim_start_matches(['*', '&']).trim();
    let ident = ident.rsplit("::").next().unwrap_or(ident);
    let (head, _) = ident.split_once("_GENERATE_ARTICLE_")?;
    let named = if let Some(fighter) = head.strip_prefix("FIGHTER_") {
        let fighter = fighter.to_lowercase();
        names.contains(&fighter).then_some(fighter)
    } else {
        let rest = head.strip_prefix("WEAPON_")?.to_lowercase();
        split_agent(&rest, names).map(|(fighter, _)| fighter)
    }?;
    (!named.eq_ignore_ascii_case(owner)).then_some(named)
}

/// Why a clone listed under `fighter` is not that fighter's, if it isn't.
pub(super) fn clone_owner_mismatch(article: &super::Article, fighter: &str) -> Option<String> {
    let ArticleOrigin::Clone(decl) = &article.origin else {
        return None;
    };
    (!decl.new_owner.eq_ignore_ascii_case(fighter)).then(|| {
        format!(
            "`{}` is listed under {fighter} but its clone_weapon names {} as its owner",
            article.agent, decl.new_owner
        )
    })
}

/// The agent names a file's text declares: `Agent::new("…")` and `agent = "…"`.
fn agent_literals(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for marker in ["Agent::new(\"", "agent = \"", "agent=\""] {
        let mut rest = text;
        while let Some(at) = rest.find(marker) {
            rest = &rest[at + marker.len()..];
            if let Some(end) = rest.find('"') {
                out.push(rest[..end].to_lowercase());
            }
        }
    }
    out
}

/// Whether a script the index puts on `agent` (`<owner>_<name>`) is written where that owner is
/// named: a file that registers it by that agent explicitly, else the nearest file or folder that
/// names any agent, which must name one of `owner`'s.
pub(super) fn script_scope_check(
    agent: &str,
    owner: &str,
    file: &Path,
    function: &str,
    root: &Path,
    sources: &[(PathBuf, String)],
    names: &[String],
) -> Result<(), String> {
    let names_function = |text: &str| {
        text.match_indices(function).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = text[at + function.len()..].chars().next();
            !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
                && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
        })
    };
    if sources
        .iter()
        .any(|(_, text)| agent_literals(text).iter().any(|a| a == agent) && names_function(text))
    {
        return Ok(());
    }
    let owned = |name: &String| split_agent(name, names).is_some_and(|(o, _)| o == owner);
    let in_file: Vec<String> = sources
        .iter()
        .filter(|(f, _)| f == file)
        .flat_map(|(_, t)| agent_literals(t))
        .collect();
    if !in_file.is_empty() {
        return if in_file.iter().any(owned) {
            Ok(())
        } else {
            Err(format!("its file names only {}", in_file.join(", ")))
        };
    }
    let mut dir = file.parent();
    while let Some(d) = dir {
        let here: Vec<String> = sources
            .iter()
            .filter(|(f, _)| f.parent() == Some(d))
            .flat_map(|(_, t)| agent_literals(t))
            .collect();
        if !here.is_empty() {
            return if here.iter().any(owned) {
                Ok(())
            } else {
                Err(format!(
                    "the nearest folder naming agents, `{}`, names only {}",
                    d.display(),
                    here.join(", ")
                ))
            };
        }
        if d == root {
            break;
        }
        dir = d.parent();
    }
    Err(format!("no file around it names {owner} or `{agent}`"))
}

/// `let NAME = EXPR;` bindings in `body`, as `resolve_id` takes them.
fn let_bindings(body: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in body.lines() {
        let Some(rest) = line.trim().strip_prefix("let ") else {
            continue;
        };
        let rest = rest.trim_start_matches("mut ").trim_start();
        let Some((name, init)) = rest.split_once('=') else {
            continue;
        };
        let name = name.split(':').next().unwrap_or("").trim();
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        out.insert(
            name.to_string(),
            init.trim().trim_end_matches(';').trim().to_string(),
        );
    }
    out
}

#[derive(Default)]
struct ArticleStats {
    rows: usize,
    spawns: usize,
    status_rows: usize,
    settings: BTreeMap<String, Vec<String>>,
    block_present: Option<bool>,
}

#[derive(Default)]
struct Moveset {
    name: String,
    root: PathBuf,
    fighters: Vec<String>,
    /// fighter -> (dump folder used, or why none)
    fighter_dirs: Vec<(String, String)>,
    articles: Vec<(super::Article, ArticleStats)>,
    clones: usize,
    unresolved_clones: Vec<String>,
    clone_notes: Vec<String>,
    ids_resolved: usize,
    ids_unresolved: Vec<String>,
    status_resolved: usize,
    status_unresolved: Vec<String>,
    unresolved_statuses: Vec<String>,
    unattributed_agents: Vec<String>,
    unread_overrides: Vec<String>,
    conflicts: Vec<String>,
    settings_by_kind: BTreeMap<&'static str, usize>,
    rt_files: BTreeSet<PathBuf>,
    rt_scripts: usize,
    rt_failed: Vec<String>,
    rt_refused: Vec<String>,
    /// Ids naming another fighter that the fighter writes on purpose (Kirby, echoes).
    cross_ids: Vec<String>,
    failures: Vec<String>,
}

fn kind_of(value: &SettingValue) -> &'static str {
    match value {
        SettingValue::Literal(_) => "literal",
        SettingValue::Const { .. } => "const",
        SettingValue::Param { .. } => "param",
        SettingValue::Computed(_) => "computed",
    }
}

fn describe(value: &SettingValue) -> String {
    match value {
        SettingValue::Literal(v) => format!("literal {v}"),
        SettingValue::Const { name, value } => match value {
            Some(v) => format!("const {name} = {v}"),
            None => format!("const {name} (value unknown)"),
        },
        SettingValue::Param { block, field } => format!("param {block}/{field}"),
        SettingValue::Computed(text) => {
            let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let short: String = flat.chars().take(70).collect();
            format!("computed `{short}`")
        }
    }
}

/// The body a save of `site` edits: the function, or the callee it forwards to when a save may
/// write there. `Err` with the reason when a save is refused.
fn save_body(site: &ScriptSite, script: &str) -> Result<(PathBuf, usize, String), String> {
    let (file, span) = match &site.delegate {
        None => (&site.file, site.span.clone()),
        Some(d) => {
            if let Some(reason) = d.write_refusal(script, &site.function) {
                return Err(reason);
            }
            (&d.file, d.span.clone())
        }
    };
    let text = std::fs::read_to_string(file).map_err(|e| format!("unreadable: {e}"))?;
    let body = text
        .get(span.clone())
        .ok_or("span out of date")?
        .to_string();
    Ok((file.clone(), line_of(&text, span.start), body))
}

/// An edit to one field of `call`, for the edit-and-revert check: its first argument that is not
/// the target, else the article.
fn edit_one(call: &ArticleCall) -> ArticleCall {
    let mut out = call.clone();
    out.edited = true;
    let target = call.op.target_index();
    let bump = |s: &str| {
        if s.trim() == "1" {
            "2".to_string()
        } else {
            "1".to_string()
        }
    };
    match (0..call.args.len()).find(|&i| Some(i) != target) {
        Some(i) => out.args[i] = bump(&call.args[i]),
        None => out.article = format!("{}_SWEEP", call.article.trim()),
    }
    out
}

/// The byte round trips of one script body, as a save runs them. Returns hard failures and
/// refusals.
fn round_trip(body: &str, label: &str) -> (Vec<String>, Vec<String>) {
    use crate::acmd_src::rewrite_editor_article_calls;
    let mut failed = Vec::new();
    let mut refused = Vec::new();
    let calls = crate::acmd::parse_acmd_script(body).to_article_calls();
    // Parse then emit: an unedited call saves its statement as written.
    for (_, call) in &calls {
        if call.edited || call.emit() != call.source {
            failed.push(format!(
                "{label}: unedited call emits `{}` for `{}`",
                call.emit(),
                call.source
            ));
        }
    }
    // An unedited save leaves the body as it is.
    match rewrite_editor_article_calls(body, label, &calls, &calls) {
        Ok((text, _)) if text == body => {}
        Ok(_) => failed.push(format!("{label}: a save with no edits changed the script")),
        Err(e) => failed.push(format!("{label}: a save with no edits failed: {e}")),
    }
    let sites = crate::acmd_src::article_call_sites(body);
    if sites.len() != calls.len() {
        refused.push(format!(
            "{label}: {} `ArticleModule` statement(s) in the source, {} read as calls — its calls cannot be saved by position",
            sites.len(),
            calls.len()
        ));
        return (failed, refused);
    }
    // Each call edited alone, saved, then edited back: only that statement may change, and
    // putting the value back restores every byte.
    for (i, site) in sites.iter().enumerate() {
        let mut edited = calls.clone();
        edited[i].1 = edit_one(&calls[i].1);
        let (text, report) = match rewrite_editor_article_calls(body, label, &calls, &edited) {
            Ok(done) => done,
            Err(e) => {
                failed.push(format!("{label}: call #{} edit failed: {e}", i + 1));
                continue;
            }
        };
        if !report.skipped.is_empty() {
            refused.push(format!(
                "{label}: call #{} edit refused: {}",
                i + 1,
                report.skipped.join("; ")
            ));
            continue;
        }
        if text == body {
            failed.push(format!("{label}: call #{} edit was not written", i + 1));
            continue;
        }
        let prefix = text
            .bytes()
            .zip(body.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        let suffix = text
            .bytes()
            .rev()
            .zip(body.bytes().rev())
            .take_while(|(a, b)| a == b)
            .count();
        if prefix < site.span.start || suffix < body.len() - site.span.end.min(body.len()) {
            failed.push(format!(
                "{label}: call #{} edit changed bytes outside its statement",
                i + 1
            ));
            continue;
        }
        let back = match rewrite_editor_article_calls(&text, label, &edited, &calls) {
            Ok((back, report)) if report.skipped.is_empty() => back,
            Ok((_, report)) => {
                failed.push(format!(
                    "{label}: call #{} revert refused: {}",
                    i + 1,
                    report.skipped.join("; ")
                ));
                continue;
            }
            Err(e) => {
                failed.push(format!("{label}: call #{} revert failed: {e}", i + 1));
                continue;
            }
        };
        if back != body {
            failed.push(format!(
                "{label}: call #{} edit then revert does not restore the script byte for byte",
                i + 1
            ));
        }
    }
    let _ = crate::project_consts::take_pending();
    let _ = crate::project_helpers::take_refused();
    (failed, refused)
}

fn sweep_one(name: &str, root: &Path, dump: Option<&Path>) -> Moveset {
    let mut out = Moveset {
        name: name.to_string(),
        root: root.to_path_buf(),
        ..Default::default()
    };
    let index = match SourceIndex::build(root) {
        Ok(index) => index,
        Err(e) => {
            out.failures.push(format!("index failed: {e:#}"));
            return out;
        }
    };
    let short = |p: &Path| super::view::short_path(p, Some(root));
    // Fighter names as the app lists them: the dump's and the moveset's own, plus every vanilla
    // name so a fighter the local dump lacks is still recognised as an owner.
    let mut names: BTreeSet<String> = crate::data::VANILLA_FIGHTERS
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Some(dump) = dump {
        names.extend(subdirs(&dump.join("fighter")));
    }
    names.extend(subdirs(&root.join("romfs").join("fighter")));
    let names: Vec<String> = names.into_iter().collect();

    // Every agent the source registers anything on.
    let mut agents: BTreeSet<String> = index.fighters.keys().cloned().collect();
    agents.extend(index.statuses.keys().cloned());
    let sources: Vec<(PathBuf, String)> = super::rust_sources(&index)
        .into_iter()
        .filter(|(_, text)| text.contains("clone_weapon") || text.contains("const "))
        .collect();
    let (clones, unresolved_clones) = super::clones::read_clones(&sources, &index.consts, &names);
    out.clones = clones.len();
    for u in &unresolved_clones {
        out.unresolved_clones.push(format!(
            "`{}:{}` `{}` — {}",
            short(&u.file),
            u.line,
            u.text.trim(),
            u.reason
        ));
    }
    for d in &clones {
        if let Some(note) = &d.id_note {
            out.clone_notes.push(format!(
                "`{}:{}` {}_{} — {note}",
                short(&d.file),
                d.line,
                d.new_owner,
                d.new_name
            ));
        }
    }
    let mut fighters: BTreeSet<String> = BTreeSet::new();
    for agent in &agents {
        match split_agent(agent, &names) {
            Some((owner, _)) => {
                fighters.insert(owner);
            }
            // smashline's agents for every fighter and every weapon, and the index's label for
            // scripts no module names a fighter for: none of them owns an article.
            None if matches!(agent.as_str(), "fighter" | "weapon" | "") => {}
            None => out
                .unattributed_agents
                .push(format!("`{agent}` names no fighter")),
        }
    }
    for d in &clones {
        fighters.insert(d.new_owner.to_lowercase());
    }
    fighters.retain(|f| !matches!(f.as_str(), "common"));
    out.fighters = fighters.iter().cloned().collect();

    let all_sources = super::rust_sources(&index);
    let mut owner_of_agent: HashMap<String, String> = HashMap::new();
    let mut listed_agents: BTreeSet<String> = BTreeSet::new();
    let no_locals = HashMap::new();
    for fighter in &fighters {
        let dump_dir = dump
            .map(|d| d.join("fighter").join(fighter))
            .filter(|d| d.is_dir());
        let fighter_dir = dump_dir
            .clone()
            .unwrap_or_else(|| root.join("romfs").join("fighter").join(fighter));
        out.fighter_dirs.push((
            fighter.clone(),
            if dump_dir.is_some() {
                "dump".into()
            } else {
                "not in the dump".into()
            },
        ));
        let catalog: ArticleCatalog = super::catalog_for(fighter, &index, &names, &fighter_dir);
        for a in &catalog.articles {
            if let Some(other) = owner_of_agent.insert(a.agent.clone(), a.owner.clone()) {
                out.failures.push(format!(
                    "`{}` is listed under both {other} and {}",
                    a.agent, a.owner
                ));
            }
            listed_agents.insert(a.agent.clone());
            if a.owner != *fighter {
                out.failures.push(format!(
                    "{fighter}'s catalog lists `{}` owned by {}",
                    a.agent, a.owner
                ));
            }
            if let Some(why) = clone_owner_mismatch(a, fighter) {
                out.failures.push(why);
            }
            // A clone's id names, and the ids the catalog answers to, name this fighter or none.
            for symbol in &a.id_symbols {
                if let Some(other) = foreign_id_owner(symbol, fighter, &names) {
                    let line = format!("`{}` answers to `{symbol}`, {other}'s id", a.agent);
                    if cross_fighter_ok(fighter, &other) {
                        out.cross_ids.push(line);
                    } else {
                        out.failures.push(line);
                    }
                }
            }
            // Each of its scripts is written where its owner is named.
            if let Some(scripts) = index.fighters.get(&a.agent) {
                for site in scripts.scripts.values() {
                    if let Err(why) = script_scope_check(
                        &a.agent,
                        fighter,
                        &site.file,
                        &site.function,
                        root,
                        &all_sources,
                        &names,
                    ) {
                        out.failures.push(format!(
                            "`{}:{}` {} is attributed to `{}`, but {why}",
                            short(&site.file),
                            line_of(
                                &std::fs::read_to_string(&site.file).unwrap_or_default(),
                                site.span.start
                            ),
                            site.function,
                            a.agent
                        ));
                    }
                }
            }
        }
        let mut stats: Vec<ArticleStats> = catalog
            .articles
            .iter()
            .map(|_| ArticleStats::default())
            .collect();
        let at = |a: &super::Article| {
            catalog
                .articles
                .iter()
                .position(|b| b.agent == a.agent)
                .unwrap()
        };

        // Scripts on the fighter and on each of its articles.
        let mut on: Vec<String> = vec![fighter.clone()];
        on.extend(catalog.articles.iter().map(|a| a.agent.clone()));
        for agent in &on {
            let Some(scripts) = index.fighters.get(agent) else {
                continue;
            };
            let mut names_sorted: Vec<&String> = scripts.scripts.keys().collect();
            names_sorted.sort();
            for script in names_sorted {
                let site = &scripts.scripts[script];
                let (file, first, body) = match save_body(site, script) {
                    Ok(found) => found,
                    Err(reason) => {
                        // Still read for ids, from the function itself.
                        let text = std::fs::read_to_string(&site.file).unwrap_or_default();
                        let body = text.get(site.span.clone()).unwrap_or("").to_string();
                        if body.contains("ArticleModule") {
                            out.rt_refused.push(format!("{agent}/{script}: {reason}"));
                        }
                        (site.file.clone(), line_of(&text, site.span.start), body)
                    }
                };
                let _agent = crate::acmd::agent_scope(&body);
                let _consts = crate::project_consts::scope(std::sync::Arc::clone(&index.consts));
                let _macros =
                    crate::hash_macros::scope(std::sync::Arc::new(index.hash_macros.clone()));
                let _helpers = crate::project_helpers::scope(std::sync::Arc::clone(&index.helpers));
                let calls = crate::acmd::parse_acmd_script(&body).to_article_calls();
                if calls.is_empty() && !body.contains("ArticleModule") {
                    continue;
                }
                let locals = let_bindings(&body);
                for (_, call) in &calls {
                    let line = body
                        .find(call.source.trim())
                        .map(|o| first + line_of(&body, o) - 1)
                        .unwrap_or(first);
                    let place = format!("`{}:{line}`", short(&file));
                    match catalog.resolve_id(&call.article, &no_locals) {
                        Some(a) => {
                            if a.owner != *fighter {
                                out.failures.push(format!(
                                    "{place} {agent}/{script}: `{}` resolves to {}'s {}",
                                    call.article, a.owner, a.name
                                ));
                            }
                            if let Some(other) = foreign_id_owner(&call.article, fighter, &names) {
                                let line = format!("{place} {agent}/{script}: `{}` names {other} but resolves to {fighter}'s {}", call.article.trim(), a.name);
                                if cross_fighter_ok(fighter, &other) {
                                    out.cross_ids.push(line)
                                } else {
                                    out.failures.push(line)
                                }
                            }
                            out.ids_resolved += 1;
                            let s = &mut stats[at(a)];
                            s.rows += 1;
                            if matches!(call.op, ArticleOp::Generate) {
                                s.spawns += 1;
                            }
                        }
                        None => {
                            let hint = match catalog.resolve_id(&call.article, &locals) {
                                Some(a) => format!("a local binding naming {}", a.name),
                                None => unresolved_hint(&call.article, fighter, dump_dir.is_some()),
                            };
                            out.ids_unresolved.push(format!(
                                "{place} {agent}/{script} `{}` ({}) — {hint}",
                                call.article.trim(),
                                call.op.name()
                            ));
                        }
                    }
                }
                if !calls.is_empty() {
                    out.rt_files.insert(file.clone());
                    out.rt_scripts += 1;
                    let label = format!("`{}:{first}` {agent}/{script}", short(&file));
                    let (failed, refused) = round_trip(&body, &label);
                    out.rt_failed.extend(failed);
                    out.rt_refused.extend(refused);
                }
            }
            // Status code: listed read-only, but its ids still resolve.
            let bodies = index.status_bodies(agent);
            let _rows = super::view::status_call_rows(&bodies, &catalog, Some(&index.root));
            for (site, body) in &bodies {
                let text = std::fs::read_to_string(&site.file).unwrap_or_default();
                let first = line_of(&text, site.span.start);
                let locals = let_bindings(body);
                for (offset, line) in body.lines().enumerate() {
                    // The rows the app lists: what `status_call_rows` reads from each line.
                    let Some(read) = super::view::status_line_call(line) else {
                        continue;
                    };
                    let place = format!("`{}:{}`", short(&site.file), first + offset);
                    let Some(call) = read else {
                        out.status_unresolved.push(format!(
                            "{place} {agent} status: not a statement Visionary reads: `{}`",
                            line.trim()
                        ));
                        continue;
                    };
                    match catalog.resolve_id(&call.article, &no_locals) {
                        Some(a) => {
                            if a.owner != *fighter {
                                out.failures.push(format!(
                                    "{place} {agent} status: `{}` resolves to {}'s {}",
                                    call.article, a.owner, a.name
                                ));
                            }
                            if let Some(other) = foreign_id_owner(&call.article, fighter, &names) {
                                let line = format!("{place} {agent} status: `{}` names {other} but resolves to {fighter}'s {}", call.article.trim(), a.name);
                                if cross_fighter_ok(fighter, &other) {
                                    out.cross_ids.push(line)
                                } else {
                                    out.failures.push(line)
                                }
                            }
                            out.status_resolved += 1;
                            stats[at(a)].status_rows += 1;
                        }
                        None => {
                            let hint = match catalog.resolve_id(&call.article, &locals) {
                                Some(a) => format!("a local binding naming {}", a.name),
                                None => unresolved_hint(&call.article, fighter, dump_dir.is_some()),
                            };
                            out.status_unresolved.push(format!(
                                "{place} {agent} status `{}` ({}) — {hint}",
                                call.article.trim(),
                                call.op.name()
                            ));
                        }
                    }
                }
            }
        }

        // Settings and param blocks, as the Settings card reads them.
        let (overrides, unread) = super::param_overrides_for(&index, fighter, &names);
        for u in unread {
            let line = format!(
                "`{}:{}` `{}` — {}",
                short(&u.file),
                u.line,
                u.text.trim(),
                u.reason
            );
            if !out.unread_overrides.contains(&line) {
                out.unread_overrides.push(line);
            }
        }
        let mut layers: Vec<PathBuf> = Vec::new();
        for base in dump
            .into_iter()
            .map(Path::to_path_buf)
            .chain([root.join("romfs")])
        {
            for file in ["vl.prc", "vl.prcxml"] {
                let path = base.join("fighter").join(fighter).join("param").join(file);
                if path.is_file() {
                    layers.push(path);
                }
            }
        }
        for (i, a) in catalog.articles.iter().enumerate() {
            let settings = super::settings_for(&index, &a.agent);
            for s in &settings {
                *out.settings_by_kind.entry(kind_of(&s.value)).or_default() += 1;
                stats[i]
                    .settings
                    .entry(s.field.label().to_string())
                    .or_default()
                    .push(describe(&s.value));
            }
            if !layers.is_empty() {
                let full = format!("param_{}", a.name);
                let set = super::params::block_overrides(&overrides, &a.owner, &full);
                let block = super::params::param_block(&layers, &set, &a.name, &HashMap::new());
                stats[i].block_present = Some(block.present);
            }
            for c in &a.conflicts {
                out.conflicts
                    .push(format!("{}/{c} is registered more than once", a.agent));
            }
        }
        out.articles
            .extend(catalog.articles.iter().cloned().zip(stats));
    }

    // Statuses the index could not attribute, and agents with scripts no catalog lists.
    for u in &index.unresolved_statuses {
        out.unresolved_statuses.push(format!(
            "`{}:{}` {} {} `{}` — {}",
            short(&u.file),
            u.line,
            u.agent_hint.as_deref().unwrap_or("(no agent)"),
            u.phase,
            u.kind_expr,
            u.reason
        ));
    }
    for agent in &agents {
        if fighters.contains(agent) || listed_agents.contains(agent) {
            continue;
        }
        if let Some((owner, Some(_))) = split_agent(agent, &names) {
            let scripts = index.fighters.get(agent).map_or(0, |f| f.scripts.len());
            let statuses = index.statuses.get(agent).map_or(0, |s| s.len());
            let why = if dump.is_some_and(|d| d.join("fighter").join(&owner).is_dir()) {
                format!("no folder for it under {owner} in the dump and no clone adds it")
            } else {
                format!("environment: {owner} is not in the local dump, so its vanilla articles are not listed")
            };
            out.unattributed_agents.push(format!(
                "`{agent}` ({scripts} scripts, {statuses} statuses): {why}"
            ));
        }
    }
    out.failures.extend(out.rt_failed.iter().cloned());
    out
}

fn unresolved_hint(expr: &str, fighter: &str, in_dump: bool) -> String {
    let ident = expr.trim().trim_start_matches(['*', '&']).trim();
    let ident = ident.rsplit("::").next().unwrap_or(ident);
    if let Some((head, name)) = ident.split_once("_GENERATE_ARTICLE_") {
        let owner = head.trim_start_matches("FIGHTER_").to_lowercase();
        if owner != fighter {
            return format!("another fighter's id ({owner}'s {})", name.to_lowercase());
        }
        if !in_dump {
            return format!("environment: {fighter} is not in the local dump, so its vanilla `{}` is not listed", name.to_lowercase());
        }
        return format!(
            "{fighter} has no `{}` folder in the dump and no clone binds this name",
            name.to_lowercase()
        );
    }
    if ident.chars().all(|c| c.is_ascii_digit() || c == '-') {
        return "a number, which depends on the build".into();
    }
    "an expression Visionary does not follow".into()
}

fn render(sets: &[Moveset], skipped: &[(String, String)], dump: Option<&Path>) -> String {
    let mut md = String::new();
    let _ = writeln!(md, "# Article corpus sweep\n");
    let _ = writeln!(
        md,
        "Dump: `{}`.\n",
        dump.map_or("none".into(), |d| d.display().to_string())
    );
    let _ = writeln!(md, "## Skipped folders\n");
    for (name, why) in skipped {
        let _ = writeln!(md, "- `{name}` — {why}");
    }
    let _ = writeln!(md, "\n## Summary\n");
    let _ = writeln!(
        md,
        "| Moveset | Articles (V/R/C) | Clones (unresolved) | Ids resolved / unresolved | Status ids resolved / unresolved | Settings lit/const/param/computed | Round trip (scripts, files) | Edits refused | Failures |"
    );
    let _ = writeln!(md, "|---|---|---|---|---|---|---|---|---|");
    for m in sets {
        let count =
            |f: fn(&ArticleOrigin) -> bool| m.articles.iter().filter(|(a, _)| f(&a.origin)).count();
        let k = |name| m.settings_by_kind.get(name).copied().unwrap_or(0);
        let _ =
            writeln!(
            md,
            "| {} | {}/{}/{} | {} ({}) | {} / {} | {} / {} | {}/{}/{}/{} | {} ({}, {}) | {} | {} |",
            m.name,
            count(|o| matches!(o, ArticleOrigin::Vanilla)),
            count(|o| matches!(o, ArticleOrigin::Rewritten)),
            count(|o| matches!(o, ArticleOrigin::Clone(_))),
            m.clones,
            m.unresolved_clones.len(),
            m.ids_resolved,
            m.ids_unresolved.len(),
            m.status_resolved,
            m.status_unresolved.len(),
            k("literal"),
            k("const"),
            k("param"),
            k("computed"),
            if m.rt_failed.is_empty() { "identical" } else { "DIFFERS" },
            m.rt_scripts,
            m.rt_files.len(),
            m.rt_refused.len(),
            m.failures.len(),
        );
    }
    for m in sets {
        let _ = writeln!(md, "\n## {}\n", m.name);
        let _ = writeln!(md, "Root `{}`. Fighters: {}.\n", m.root.display(), {
            m.fighter_dirs
                .iter()
                .map(|(f, d)| format!("{f} ({d})"))
                .collect::<Vec<_>>()
                .join(", ")
        });
        let _ = writeln!(md, "| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |");
        let _ = writeln!(md, "|---|---|---|---|---|---|---|---|");
        for (a, s) in &m.articles {
            let origin = match &a.origin {
                ArticleOrigin::Vanilla => "vanilla".to_string(),
                ArticleOrigin::Rewritten => "rewritten".to_string(),
                ArticleOrigin::Clone(d) => format!(
                    "clone of {}/{}{}",
                    d.src_owner.as_deref().unwrap_or("?"),
                    d.src_kind,
                    if d.use_original_code {
                        ", original code"
                    } else {
                        ""
                    }
                ),
            };
            // Vanilla articles with nothing in the moveset are listed in the count only.
            if matches!(a.origin, ArticleOrigin::Vanilla)
                && s.rows == 0
                && s.status_rows == 0
                && s.settings.is_empty()
            {
                continue;
            }
            let settings = s
                .settings
                .iter()
                .map(|(f, v)| format!("{f}: {}", v.join(", ")))
                .collect::<Vec<_>>()
                .join("; ");
            let _ = writeln!(
                md,
                "| {} | {} | {} | {} | {} ({}) | {} | {} | {} |",
                a.agent,
                origin,
                if a.id_symbols.is_empty() {
                    a.id_note.clone().unwrap_or("none".into())
                } else {
                    a.id_symbols.join(", ")
                },
                a.source_scripts.len(),
                s.rows,
                s.spawns,
                s.status_rows,
                match s.block_present {
                    Some(true) => "present",
                    Some(false) => "absent",
                    None => "no param file",
                },
                settings.replace('|', "\\|"),
            );
        }
        let lists: [(&str, &Vec<String>); 12] = [
            ("Ids that name another fighter on purpose", &m.cross_ids),
            ("Failures", &m.failures),
            ("Unresolved ids (scripts)", &m.ids_unresolved),
            ("Unresolved ids (status code)", &m.status_unresolved),
            ("Unresolved clones", &m.unresolved_clones),
            ("Clone ids not bound", &m.clone_notes),
            ("Unresolved statuses", &m.unresolved_statuses),
            ("Agents no catalog lists", &m.unattributed_agents),
            ("Unread param_config overrides", &m.unread_overrides),
            ("Edits refused", &m.rt_refused),
            ("Registered twice", &m.conflicts),
            ("Round trip differences", &m.rt_failed),
        ];
        for (title, items) in lists {
            if items.is_empty() {
                continue;
            }
            let _ = writeln!(md, "\n### {title} ({})\n", items.len());
            for item in items {
                let _ = writeln!(md, "- {}", item.replace('\n', " "));
            }
        }
    }
    md
}

#[test]
#[ignore = "reads every moveset under VISIONARY_MOVESETS; run with --ignored"]
fn corpus_sweep_reports_every_article() {
    let movesets = std::env::var_os("VISIONARY_MOVESETS")
        .map(PathBuf::from)
        .expect("set VISIONARY_MOVESETS to the folder holding the movesets");
    let dump = dump_root();
    let mut skipped = Vec::new();
    let mut sets = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(&movesets)
        .expect("VISIONARY_MOVESETS is readable")
        .flatten()
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if !entry.path().is_dir() {
            if name.ends_with(".zip") {
                skipped.push((name, "an archive, not a folder".to_string()));
            }
            continue;
        }
        if let Some(why) = skip_reason(&name) {
            skipped.push((name, why.to_string()));
            continue;
        }
        let Some(root) = crate_root(&entry.path()) else {
            skipped.push((name, "no Cargo.toml".to_string()));
            continue;
        };
        eprintln!("sweeping {name} at {}", root.display());
        let path = root.clone();
        let dump_ref = dump.clone();
        let label = name.clone();
        let swept = std::panic::catch_unwind(move || sweep_one(&label, &path, dump_ref.as_deref()));
        match swept {
            Ok(m) => sets.push(m),
            Err(panic) => {
                let why = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                sets.push(Moveset {
                    name,
                    root,
                    failures: vec![format!("the sweep panicked: {why}")],
                    ..Default::default()
                });
            }
        }
    }
    let md = render(&sets, &skipped, dump.as_deref());
    let out = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("article-corpus.md");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    std::fs::write(&out, &md).unwrap();
    eprintln!("report written to {}", out.display());
    let failures: Vec<String> = sets
        .iter()
        .flat_map(|m| m.failures.iter().map(move |f| format!("{}: {f}", m.name)))
        .collect();
    let residue: usize = sets
        .iter()
        .map(|m| {
            m.ids_unresolved.len()
                + m.status_unresolved.len()
                + m.unresolved_clones.len()
                + m.clone_notes.len()
                + m.unresolved_statuses.len()
                + m.unattributed_agents.len()
                + m.unread_overrides.len()
                + m.rt_refused.len()
        })
        .sum();
    eprintln!(
        "{} movesets, {} failures, {residue} residue items",
        sets.len(),
        failures.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_splits_by_longest_fighter_name() {
        let names: Vec<String> = ["mario", "mariod", "pacman"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            split_agent("mariod_drcapsule", &names),
            Some(("mariod".into(), Some("drcapsule".into())))
        );
        assert_eq!(split_agent("mario", &names), Some(("mario".into(), None)));
        assert_eq!(split_agent("pacman_", &names), None);
        assert_eq!(split_agent("toad", &names), None);
    }

    /// I-1: the misattribution checks catch a deliberate wrong attribution.
    #[test]
    fn wrong_attributions_fail_the_checks() {
        let names: Vec<String> = ["mario", "samus", "kirby", "dedede", "murabito"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // An id naming another fighter.
        assert_eq!(
            foreign_id_owner("*FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL", "samus", &names)
                .as_deref(),
            Some("mario")
        );
        assert_eq!(
            foreign_id_owner("FIGHTER_SAMUS_GENERATE_ARTICLE_LATIVOLT", "Samus", &names),
            None
        );
        assert_eq!(
            foreign_id_owner(
                "WEAPON_MURABITO_SLINGSHOT_GENERATE_ARTICLE_BULLET",
                "mario",
                &names
            )
            .as_deref(),
            Some("murabito")
        );
        assert_eq!(
            foreign_id_owner(
                "WEAPON_MURABITO_SLINGSHOT_GENERATE_ARTICLE_BULLET",
                "murabito",
                &names
            ),
            None
        );
        // A moveset's own name for a fighter (King Boo on dedede) names nobody else.
        assert_eq!(
            foreign_id_owner(
                "FIGHTER_KINGTERESA_GENERATE_ARTICLE_FIREBALL",
                "dedede",
                &names
            ),
            None
        );
        assert!(cross_fighter_ok("kirby", "mario") && cross_fighter_ok("samusd", "samus"));
        assert!(!cross_fighter_ok("samus", "lucario"));

        // A clone listed under the wrong fighter.
        let decl = super::super::clones::CloneDecl {
            new_owner: "samus".into(),
            new_name: "lativolt".into(),
            src_owner: Some("mario".into()),
            src_kind: "WEAPON_KIND_MARIO_FIREBALL".into(),
            use_original_code: false,
            id_bindings: vec![],
            kind_bindings: vec![],
            id_note: None,
            file: PathBuf::from("lib.rs"),
            line: 1,
        };
        let article = super::super::Article {
            owner: "mario".into(),
            name: "lativolt".into(),
            agent: "mario_lativolt".into(),
            origin: ArticleOrigin::Clone(decl),
            id_symbols: vec![],
            source_scripts: vec![],
            conflicts: vec![],
            id_note: None,
            also_declared: vec![],
            clone_conflicts: vec![],
        };
        assert!(clone_owner_mismatch(&article, "mario").is_some());
        assert!(clone_owner_mismatch(&article, "Samus").is_none());

        // A script put on `mario_fireball` from a folder that only names luigi.
        let root = PathBuf::from("r");
        let sources = vec![
            (root.join("luigi/mod.rs"), "pub fn install() { let a = &mut Agent::new(\"luigi\"); acmd::install(a); }".to_string()),
            (root.join("luigi/acmd.rs"), "unsafe extern \"C\" fn game_fly(agent: &mut L2CAgentBase) {}".to_string()),
            (root.join("mario/mod.rs"), "pub fn install() { Agent::new(\"mario_fireball\").acmd(\"game_x\", fireball::game_x).install(); }".to_string()),
            (root.join("mario/fireball.rs"), "unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {}".to_string()),
        ];
        let names: Vec<String> = ["mario", "luigi"].iter().map(|s| s.to_string()).collect();
        let wrong = script_scope_check(
            "mario_fireball",
            "mario",
            &root.join("luigi/acmd.rs"),
            "game_fly",
            &root,
            &sources,
            &names,
        );
        assert!(wrong.unwrap_err().contains("luigi"));
        // Registered by that agent explicitly, or under a folder naming the owner: fine.
        assert!(script_scope_check(
            "mario_fireball",
            "mario",
            &root.join("mario/fireball.rs"),
            "game_x",
            &root,
            &sources,
            &names
        )
        .is_ok());
        assert!(script_scope_check(
            "luigi_fireball",
            "luigi",
            &root.join("luigi/acmd.rs"),
            "game_fly",
            &root,
            &sources,
            &names
        )
        .is_ok());
    }

    /// Every object call in the game's own scripts edits and reverts byte for byte, or is refused
    /// (a call in a loop has no one line of its own). Set `VISIONARY_SCRIPT_DUMP` to the script
    /// dump's `smashline` folder to run it.
    #[test]
    fn every_dumped_object_call_round_trips() {
        let Some(root) = std::env::var_os("VISIONARY_SCRIPT_DUMP").map(PathBuf::from) else {
            return;
        };
        let (mut scripts, mut failures) = (0, Vec::new());
        for package in subdirs(&root) {
            for agent in subdirs(&root.join(&package)) {
                for file in std::fs::read_dir(root.join(&package).join(&agent))
                    .unwrap()
                    .flatten()
                {
                    let Ok(text) = std::fs::read_to_string(file.path()) else {
                        continue;
                    };
                    for function in text.split("unsafe extern \"C\" fn ").skip(1) {
                        if !function.contains("ArticleModule::") {
                            continue;
                        }
                        let name = function.split('(').next().unwrap_or_default();
                        let body = format!("unsafe extern \"C\" fn {function}");
                        let (failed, refused) = round_trip(&body, &format!("{agent}/{name}"));
                        failures.extend(failed);
                        scripts += usize::from(refused.is_empty());
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
        assert!(scripts > 1000, "{scripts} scripts");
    }

    #[test]
    fn edit_and_revert_keeps_every_other_byte() {
        let body = "unsafe extern \"C\" fn game_a(agent: &mut L2CAgentBase) {\r\n    frame(agent.lua_state_agent, 3.0);\r\n    if macros::is_excute(agent) {\r\n        ArticleModule::generate_article(agent.module_accessor, *FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL, false, -1);\r\n        ArticleModule::shoot(agent.module_accessor, *FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL), false);\r\n    }\r\n}\r\n";
        let (failed, refused) = round_trip(body, "game_a");
        assert!(failed.is_empty(), "{failed:#?}");
        assert!(refused.is_empty(), "{refused:#?}");
    }
}
