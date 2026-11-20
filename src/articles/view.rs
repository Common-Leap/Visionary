//! UI-free row models for a fighter's objects: the Objects list, the plain-words timeline rows,
//! the Settings card and the read-only rows for article calls made from status code.
//!
//! Every value a row shows carries where it comes from (`origin`), and a value Visionary cannot
//! write safely is marked not editable rather than hidden.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::clone_compat;
use super::params::{ParamBlock, ParamValue};
use super::settings::{Setting, SettingField, SettingValue};
use super::{Article, ArticleCatalog, ArticleOrigin};
use crate::acmd_src::StatusSite;
use crate::data::{ArticleCall, ArticleOp, ArticleTarget};
use crate::project_consts::ProjectConsts;

/// Shown beside the variant picker when a param block is a list.
pub const VARIANT_NOTE: &str = "This block is a list. Which entry the game reads for a two-hash \
    `get_param_*` call isn't confirmed yet, so pick the one you mean to see and change.";

/// One article in the Objects list.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectRow {
    pub title: String,
    /// "Game's", "Rewritten", or "Clone of Mario's fireball".
    pub badge: String,
    pub warnings: Vec<String>,
    /// The article's scripts as moves (`fly` for `game_fly` and `effect_fly`), sorted.
    pub scripts: Vec<String>,
}

fn capitalized(word: &str) -> String {
    let mut letters = word.chars();
    letters
        .next()
        .map_or_else(String::new, |first| first.to_uppercase().chain(letters).collect())
}

/// `WEAPON_KIND_MARIO_FIREBALL` with owner `mario` gives `fireball`.
fn source_article_name(kind: &str, owner: Option<&str>) -> String {
    let kind = kind.trim_start_matches('*');
    let kind = kind.strip_prefix("WEAPON_KIND_").unwrap_or(kind);
    let rest = owner
        .and_then(|owner| kind.strip_prefix(&format!("{}_", owner.to_uppercase())))
        .unwrap_or(kind);
    rest.to_lowercase()
}

/// The move name a script is listed under: its category prefix removed.
pub fn script_move_name(script: &str) -> &str {
    for prefix in ["game_", "effect_", "sound_", "expression_"] {
        if let Some(rest) = script.strip_prefix(prefix) {
            return rest;
        }
    }
    script
}

pub fn badge(article: &Article) -> String {
    match &article.origin {
        ArticleOrigin::Vanilla => "Game's".into(),
        ArticleOrigin::Rewritten => "Rewritten".into(),
        ArticleOrigin::Clone(decl) => match decl.src_owner.as_deref() {
            Some(owner) => format!(
                "Clone of {}'s {}",
                capitalized(owner),
                source_article_name(&decl.src_kind, Some(owner))
            ),
            None => format!("Clone of `{}`", decl.src_kind),
        },
    }
}

pub fn object_rows(catalog: &ArticleCatalog) -> Vec<ObjectRow> {
    catalog
        .articles
        .iter()
        .map(|article| {
            let mut warnings = Vec::new();
            if let Some(note) = &article.id_note {
                warnings.push(format!("Visionary can't follow this object's id: {note}"));
            }
            if let ArticleOrigin::Clone(decl) = &article.origin {
                if let Some(entry) = clone_compat::lookup(&decl.src_kind) {
                    if entry.can_clone == Some(false) {
                        warnings.push(format!(
                            "The clone sheet marks `{}` as not cloneable (verified by {})",
                            entry.weapon_kind, entry.verified_by
                        ));
                    }
                }
            }
            for script in &article.conflicts {
                warnings.push(format!(
                    "`{script}` is registered more than once, so Visionary doesn't pick one; \
                     remove the extra registration to edit it"
                ));
            }
            let mut scripts: Vec<String> = article
                .source_scripts
                .iter()
                .map(|s| script_move_name(s).to_string())
                .collect();
            scripts.sort();
            scripts.dedup();
            ObjectRow { title: article.name.clone(), badge: badge(article), warnings, scripts }
        })
        .collect()
}

/// What the clone sheet says about a clone's source kind: its self-destruct behaviour and
/// notes, or the sheet's rule of thumb for a kind it doesn't list. Empty for a non-clone.
pub fn clone_notes(article: &Article) -> Vec<String> {
    let ArticleOrigin::Clone(decl) = &article.origin else {
        return Vec::new();
    };
    let mut out = vec![format!(
        "Cloned at {}:{}{}",
        decl.file.display(),
        decl.line,
        if decl.use_original_code { ", running the original's code" } else { "" }
    )];
    match clone_compat::lookup(&decl.src_kind) {
        Some(entry) => {
            let said = |text: &str| {
                if text.trim().is_empty() {
                    "not recorded".to_string()
                } else {
                    text.trim().to_string()
                }
            };
            out.push(format!(
                "Clone sheet, `{}`: goes away on hitting an opponent: {}; on hitting a shield: {}",
                entry.weapon_kind,
                said(entry.on_hit),
                said(entry.on_shield)
            ));
            if !entry.notes.trim().is_empty() {
                out.push(format!("Clone sheet notes: {}", entry.notes.trim()));
            }
            if !entry.verified_by.trim().is_empty() {
                out.push(format!("Verified by {}", entry.verified_by.trim()));
            }
        }
        None => out.push(format!(
            "The clone sheet doesn't list `{}`. Its advice: {}",
            decl.src_kind,
            clone_compat::ADVICE
        )),
    }
    out
}

/// The sentence for a param block an article reads but its owner's file doesn't have.
pub fn missing_block_note(article: &Article, block: &ParamBlock, settings: &[Setting]) -> Option<String> {
    if block.present {
        return None;
    }
    let full = format!("param_{}", article.name);
    let reads = settings.iter().any(|s| {
        matches!(&s.value, SettingValue::Param { block, .. } if *block == full)
            || s.referenced_params.iter().any(|(b, _)| *b == full)
    });
    let original_code = matches!(&article.origin, ArticleOrigin::Clone(d) if d.use_original_code);
    if !(reads || original_code) {
        return None;
    }
    let mut note = format!(
        "Reads params from `{full}`, which `{}`'s param file doesn't have.",
        article.owner
    );
    if matches!(article.origin, ArticleOrigin::Clone(_)) {
        note.push_str(" The original's values are not used.");
    }
    Some(note)
}

/// The article an id expression names: through the catalog, else the name a vanilla
/// `FIGHTER_X_GENERATE_ARTICLE_Y` constant spells.
fn article_name(expr: &str, catalog: &ArticleCatalog) -> Option<String> {
    if let Some(article) = catalog.resolve_id(expr, &HashMap::new()) {
        return Some(article.name.clone());
    }
    let ident = expr.trim().trim_start_matches(['*', '&']).trim();
    let ident = ident.rsplit("::").next().unwrap_or(ident);
    let (head, name) = ident.split_once("_GENERATE_ARTICLE_")?;
    (head.starts_with("FIGHTER_")
        && !name.is_empty()
        && ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    .then(|| name.to_lowercase())
}

/// "the fireball", "every fireball", or the unresolved form with the expression as written.
fn phrase(det: &str, name: Option<&str>, expr: &str) -> String {
    match name {
        Some(name) => format!("{det} {name}"),
        None if det == "the" => format!("an object Visionary can't name (`{}`)", expr.trim()),
        None => format!("{det} object Visionary can't name (`{}`)", expr.trim()),
    }
}

fn target_det(target: ArticleTarget) -> &'static str {
    match target {
        ArticleTarget::All => "every",
        ArticleTarget::Last => "the last",
        ArticleTarget::First => "the first",
        _ => "the",
    }
}

/// The text inside `Hash40::new("fly")` or `hash40("fly")`, else the argument as written.
fn hash_text(arg: &str) -> String {
    let arg = arg.trim();
    match (arg.find('"'), arg.rfind('"')) {
        (Some(open), Some(close)) if close > open => arg[open + 1..close].to_string(),
        _ => arg.to_string(),
    }
}

/// One article call in plain words: "Creates the fireball", "Shoots the last fireball".
pub fn timeline_label(call: &ArticleCall, catalog: &ArticleCatalog) -> String {
    let name = article_name(&call.article, catalog);
    let the = |det: &str| phrase(det, name.as_deref(), &call.article);
    let arg = |n: usize| call.args.get(n).map(|a| a.trim().to_string()).unwrap_or_default();
    match &call.op {
        ArticleOp::Generate => format!("Creates {}", the("the")),
        ArticleOp::GenerateEnable => format!("Lets the fighter create {}", the("the")),
        ArticleOp::Remove | ArticleOp::RemoveExist => {
            format!("Removes {}", the(target_det(call.target)))
        }
        ArticleOp::Shoot | ArticleOp::ShootExist => format!("Shoots {}", the(target_det(call.target))),
        ArticleOp::ChangeMotion => format!("Plays `{}` on {}", hash_text(&arg(0)), the("the")),
        ArticleOp::ChangeStatus | ArticleOp::ChangeStatusExist => {
            format!("Changes the status of {} to `{}`", the("the"), arg(0))
        }
        ArticleOp::SetRate => format!("Sets the animation speed of {} to {}", the("the"), arg(0)),
        ArticleOp::SetFrame => format!("Sets the animation frame of {} to {}", the("the"), arg(0)),
        ArticleOp::AddMotionPartial => format!("Plays a partial motion on {}", the("the")),
        // "Hides the fireball" for every live one too: hiding reads as a whole-object switch.
        ArticleOp::SetVisibilityWhole => match (arg(0).as_str(), call.target) {
            ("false", ArticleTarget::All) => format!("Hides {}", the("the")),
            ("true", ArticleTarget::All) => format!("Shows {}", the("the")),
            ("false", target) => format!("Hides {}", the(target_det(target))),
            ("true", target) => format!("Shows {}", the(target_det(target))),
            (other, _) => format!("Sets whether {} shows (`{other}`)", the("the")),
        },
        ArticleOp::SetVisibility => format!("Changes which parts of {} show", the("the")),
        ArticleOp::SetPos => format!("Moves {}", the("the")),
        ArticleOp::SetFlag | ArticleOp::SetInt | ArticleOp::SetFloat => {
            format!("Sets a stored value on {}", the("the"))
        }
        ArticleOp::Other(function) => {
            format!("Calls `ArticleModule::{function}` on {}", the("the"))
        }
    }
}

/// One row of an article's Settings card.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingRow {
    pub label: String,
    pub value: String,
    /// Where the value comes from: "param_lativolt › speed (samus vl.prc)", "FIREBALL_SPEED in
    /// consts.rs", "status/fly.rs:41", "worked out: speed_x * lr".
    pub origin: String,
    pub editable: bool,
}

/// What a Settings card reads beside the settings and the param block.
#[derive(Clone, Copy, Default)]
pub struct SettingContext<'a> {
    /// The list entry the card shows and edits (0 for a plain struct).
    pub variant: usize,
    /// The project's consts, to show where a const is defined.
    pub consts: Option<&'a ProjectConsts>,
    /// The project root, so file origins read relative to it.
    pub root: Option<&'a Path>,
}

/// [`setting_rows_in`] for variant 0, with no consts or root: the form the card's tests read.
#[cfg_attr(not(test), allow(dead_code))]
pub fn setting_rows(settings: &[Setting], params: &ParamBlock) -> Vec<SettingRow> {
    setting_rows_in(settings, params, SettingContext::default())
}

fn number_text(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

fn param_text(value: &ParamValue) -> String {
    match value {
        ParamValue::Float(v) => number_text(*v as f64),
        ParamValue::Int(v) => v.to_string(),
        ParamValue::Other(text) => text.clone(),
    }
}

/// `path` relative to `root`, else its last two components, with `/` separators.
pub fn short_path(path: &Path, root: Option<&Path>) -> String {
    let parts = |p: &Path| -> Vec<String> {
        p.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect()
    };
    match root.and_then(|root| path.strip_prefix(root).ok()) {
        Some(rel) => parts(rel).join("/"),
        None => {
            let all = parts(path);
            all[all.len().saturating_sub(2)..].join("/")
        }
    }
}

type Texts = HashMap<PathBuf, Option<String>>;

/// The 1-based line `byte` falls on in `file`, read from disk (cached in `texts`).
fn line_of(file: &Path, byte: usize, texts: &mut Texts) -> Option<usize> {
    let text = texts
        .entry(file.to_path_buf())
        .or_insert_with(|| std::fs::read_to_string(file).ok());
    let text = text.as_ref()?;
    let byte = byte.min(text.len());
    Some(text.as_bytes()[..byte].iter().filter(|b| **b == b'\n').count() + 1)
}

fn site_text(file: &Path, byte: usize, root: Option<&Path>, texts: &mut Texts) -> String {
    match line_of(file, byte, texts) {
        Some(line) => format!("{}:{line}", short_path(file, root)),
        None => format!("{} (byte {byte})", short_path(file, root)),
    }
}

/// `*WEAPON_X_STATUS_KIND_SHOOT` gives `shoot`; empty (an `on_start`) gives `on start`.
pub fn status_word(kind_expr: &str) -> String {
    let kind = kind_expr.trim().trim_start_matches('*');
    if kind.is_empty() {
        return "on start".into();
    }
    let kind = kind.rsplit("::").next().unwrap_or(kind);
    kind.split_once("STATUS_KIND_").map_or(kind, |(_, rest)| rest).to_lowercase()
}

/// Where a param value was read: `samus vl.prc`, or `param_config` for an override.
fn layer_name(file: &Path) -> String {
    if file.as_os_str().is_empty() {
        return "param_config".into();
    }
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match file.parent().and_then(Path::parent).and_then(Path::file_name) {
        Some(owner) => format!("{} {name}", owner.to_string_lossy()),
        None => name,
    }
}

pub fn setting_rows_in(settings: &[Setting], params: &ParamBlock, cx: SettingContext) -> Vec<SettingRow> {
    let mut texts = Texts::new();
    let fields = params.variants.get(cx.variant).unwrap_or(&params.fields);
    settings
        .iter()
        .map(|setting| {
            let label = format!("{} · {}", setting.field.label(), status_word(&setting.status));
            let facing = if setting.facing_factor { " × facing" } else { "" };
            let at = site_text(&setting.file, setting.span.start, cx.root, &mut texts);
            if setting.field == SettingField::OtherMovement {
                let text = match &setting.value {
                    SettingValue::Computed(text) => text.clone(),
                    other => format!("{other:?}"),
                };
                return SettingRow {
                    label,
                    value: "kept as written".into(),
                    origin: format!("{at}: {text}"),
                    editable: false,
                };
            }
            match &setting.value {
                SettingValue::Literal(value) => SettingRow {
                    label,
                    value: format!("{}{facing}", number_text(*value)),
                    origin: at,
                    editable: true,
                },
                SettingValue::Const { name, value } => {
                    let def = cx.consts.and_then(|consts| consts.get(name));
                    let value = def.and_then(|d| d.number()).or(*value);
                    SettingRow {
                        label,
                        value: match value {
                            Some(v) => format!("{}{facing}", number_text(v)),
                            None => "not a number this project defines".into(),
                        },
                        origin: match def {
                            Some(def) => format!("{name} in {}", short_path(&def.file, cx.root)),
                            None => format!("{name} (used at {at})"),
                        },
                        editable: value.is_some(),
                    }
                }
                SettingValue::Param { block, field } => {
                    let place = format!("{block} › {field}");
                    if !params.present {
                        return SettingRow {
                            label,
                            value: "missing".into(),
                            origin: format!("{place} (no param file has this block)"),
                            editable: false,
                        };
                    }
                    match fields.get(field) {
                        Some((value, file)) => SettingRow {
                            label,
                            value: format!("{}{facing}", param_text(value)),
                            origin: format!("{place} ({})", layer_name(file)),
                            editable: matches!(value, ParamValue::Float(_) | ParamValue::Int(_)),
                        },
                        None => SettingRow {
                            label,
                            value: "not set".into(),
                            origin: format!("{place} (no param file sets this field)"),
                            editable: false,
                        },
                    }
                }
                SettingValue::Computed(text) => SettingRow {
                    label,
                    value: "worked out".into(),
                    origin: format!(
                        "worked out: {text}{}",
                        if setting.facing_factor { " * lr" } else { "" }
                    ),
                    editable: false,
                },
            }
        })
        .collect()
}

/// The fields of a param block nothing in the status code reads, so the card still shows them.
pub fn unread_param_rows(settings: &[Setting], params: &ParamBlock, block_name: &str, variant: usize) -> Vec<SettingRow> {
    let fields = params.variants.get(variant).unwrap_or(&params.fields);
    fields
        .iter()
        .filter(|(field, _)| {
            !settings.iter().any(|s| {
                matches!(&s.value, SettingValue::Param { block, field: f } if block == block_name && f == *field)
            })
        })
        .map(|(field, (value, file))| SettingRow {
            label: field.clone(),
            value: param_text(value),
            origin: format!("{block_name} › {field} ({})", layer_name(file)),
            editable: matches!(value, ParamValue::Float(_) | ParamValue::Int(_)),
        })
        .collect()
}

/// An article call made from status code: shown under the move, never edited there.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusCallRow {
    pub label: String,
    /// "in status code, `status/special_n.rs:41`".
    pub origin: String,
    pub editable: bool,
    /// The status kind as written.
    pub status: String,
    pub file: PathBuf,
    pub line: usize,
}

/// Every `ArticleModule` statement in `bodies` (status functions with their text), one row each.
/// A line that names `ArticleModule::` but isn't a whole statement Visionary reads is still
/// listed, as kept.
pub fn status_call_rows(
    bodies: &[(&StatusSite, String)],
    catalog: &ArticleCatalog,
    root: Option<&Path>,
) -> Vec<StatusCallRow> {
    let mut texts = Texts::new();
    let mut out = Vec::new();
    for (site, body) in bodies {
        let Some(first) = line_of(&site.file, site.span.start, &mut texts) else {
            continue;
        };
        for (offset, line) in body.lines().enumerate() {
            if !line.contains("ArticleModule::") {
                continue;
            }
            let number = first + offset;
            let label = match ArticleCall::parse(line) {
                Some(call) => timeline_label(&call, catalog),
                None => format!(
                    "An object call Visionary doesn't read, kept as written: `{}`",
                    line.trim()
                ),
            };
            out.push(StatusCallRow {
                label,
                origin: format!("in status code, `{}:{number}`", short_path(&site.file, root)),
                editable: false,
                status: site.kind_expr.clone(),
                file: site.file.clone(),
                line: number,
            });
        }
    }
    out
}

/// Whether a status kind belongs to a move: `SPECIAL_N` statuses (`SPECIAL_N_C` too) go with
/// every `special_n` and `special_air_n` move; any other with the moves its name starts.
pub fn status_matches_move(kind_expr: &str, move_name: &str) -> bool {
    if kind_expr.trim().is_empty() {
        return false;
    }
    let normal = |s: &str| s.to_lowercase().replace("_air_", "_").replace("air_", "");
    let key = normal(&status_word(kind_expr));
    let family = if key.starts_with("special_") {
        key.splitn(3, '_').take(2).collect::<Vec<_>>().join("_")
    } else {
        key
    };
    let name = normal(move_name);
    name == family || name.starts_with(&format!("{family}_"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::articles::clones::CloneDecl;
    use crate::articles::settings::{SettingField, SettingValue};
    use crate::articles::{Article, ArticleOrigin};

    fn clone_of(kind: &str, owner: &str, name: &str) -> Article {
        let decl = CloneDecl {
            new_owner: "samus".into(),
            new_name: name.into(),
            src_owner: Some(owner.into()),
            src_kind: kind.into(),
            use_original_code: false,
            id_bindings: vec![format!("FIGHTER_SAMUS_GENERATE_ARTICLE_{}", name.to_uppercase())],
            kind_bindings: vec![],
            id_note: None,
            file: PathBuf::from("src/lib.rs"),
            line: 3,
        };
        Article {
            owner: "samus".into(),
            name: name.into(),
            agent: format!("samus_{name}"),
            origin: ArticleOrigin::Clone(decl.clone()),
            id_symbols: decl.id_bindings.clone(),
            source_scripts: vec!["game_fly".into(), "effect_fly".into()],
            conflicts: vec![],
            id_note: None,
        }
    }

    fn catalog(articles: Vec<Article>) -> ArticleCatalog {
        ArticleCatalog { articles, unresolved_clones: vec![] }
    }

    fn vanilla(owner: &str, name: &str) -> Article {
        Article {
            owner: owner.into(),
            name: name.into(),
            agent: format!("{owner}_{name}"),
            origin: ArticleOrigin::Vanilla,
            id_symbols: vec![format!(
                "FIGHTER_{}_GENERATE_ARTICLE_{}",
                owner.to_uppercase(),
                name.to_uppercase()
            )],
            source_scripts: vec![],
            conflicts: vec![],
            id_note: None,
        }
    }

    fn setting(value: SettingValue, file: PathBuf, span: std::ops::Range<usize>) -> Setting {
        Setting {
            field: SettingField::SpeedX,
            value,
            status: "*WEAPON_SAMUS_LATIVOLT_STATUS_KIND_SHOOT".into(),
            phase: "Main".into(),
            file,
            span,
            facing_factor: false,
            referenced_params: vec![],
        }
    }

    #[test]
    fn clone_badge_names_the_source() {
        let rows = object_rows(&catalog(vec![clone_of("WEAPON_KIND_MARIO_FIREBALL", "mario", "lativolt")]));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "lativolt");
        assert_eq!(rows[0].badge, "Clone of Mario's fireball");
        assert!(rows[0].warnings.is_empty(), "{:?}", rows[0].warnings);
        assert_eq!(rows[0].scripts, ["fly"]);
    }

    #[test]
    fn vanilla_and_rewritten_badges() {
        let mut rewritten = vanilla("falco", "cannon");
        rewritten.origin = ArticleOrigin::Rewritten;
        let rows = object_rows(&catalog(vec![vanilla("mario", "fireball"), rewritten]));
        assert_eq!(rows[0].badge, "Game's");
        assert_eq!(rows[1].badge, "Rewritten");
    }

    #[test]
    fn uncloneable_kind_warns_with_verifier() {
        let rows = object_rows(&catalog(vec![clone_of("WEAPON_KIND_BRAVE_CRASH", "brave", "crash2")]));
        assert_eq!(rows[0].warnings.len(), 1, "{:?}", rows[0].warnings);
        assert!(rows[0].warnings[0].contains("Hudson6CO"), "{}", rows[0].warnings[0]);
        assert_eq!(
            rows[0].warnings[0],
            "The clone sheet marks `BRAVE_CRASH` as not cloneable (verified by Hudson6CO)"
        );
    }

    #[test]
    fn id_note_is_a_warning() {
        let mut article = clone_of("WEAPON_KIND_MARIO_FIREBALL", "mario", "lativolt");
        article.id_note = Some("stored in a struct field".into());
        let rows = object_rows(&catalog(vec![article]));
        assert!(rows[0].warnings.iter().any(|w| w.contains("stored in a struct field")));
    }

    #[test]
    fn timeline_labels_read_in_plain_words() {
        let c = catalog(vec![vanilla("mario", "fireball")]);
        let label = |line: &str| timeline_label(&ArticleCall::parse(line).unwrap(), &c);
        assert_eq!(
            label("ArticleModule::shoot(agent.module_accessor, *FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL), false);"),
            "Shoots every fireball"
        );
        assert_eq!(
            label("ArticleModule::shoot(agent.module_accessor, *FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL, ArticleOperationTarget(*ARTICLE_OPE_TARGET_LAST), false);"),
            "Shoots the last fireball"
        );
        assert_eq!(
            label("ArticleModule::generate_article(agent.module_accessor, *FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL, false, -1);"),
            "Creates the fireball"
        );
        assert_eq!(
            label("ArticleModule::remove_exist(agent.module_accessor, *FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));"),
            "Removes every fireball"
        );
        assert_eq!(
            label("ArticleModule::change_motion(agent.module_accessor, *FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL, Hash40::new(\"fly\"), false, -1.0);"),
            "Plays `fly` on the fireball"
        );
        assert_eq!(
            label("ArticleModule::set_visibility_whole(agent.module_accessor, *FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL, false, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));"),
            "Hides the fireball"
        );
        assert_eq!(
            label("ArticleModule::generate_article(agent.module_accessor, owner_article(x), false, -1);"),
            "Creates an object Visionary can't name (`owner_article(x)`)"
        );
    }

    #[test]
    fn computed_setting_is_not_editable() {
        let s = setting(SettingValue::Computed("speed_x".into()), PathBuf::from("f.rs"), 0..7);
        let s = Setting { facing_factor: true, ..s };
        let rows = setting_rows(&[s], &ParamBlock::default());
        assert!(!rows[0].editable);
        assert!(rows[0].origin.starts_with("worked out:"), "{}", rows[0].origin);
        assert_eq!(rows[0].origin, "worked out: speed_x * lr");
    }

    #[test]
    fn missing_block_reads_missing() {
        let s = setting(
            SettingValue::Param { block: "param_lativolt".into(), field: "speed".into() },
            PathBuf::from("f.rs"),
            0..1,
        );
        let rows = setting_rows(&[s], &ParamBlock::default());
        assert_eq!(rows[0].value, "missing");
        assert!(!rows[0].editable);
        assert!(rows[0].origin.starts_with("param_lativolt › speed"), "{}", rows[0].origin);
    }

    #[test]
    fn param_and_literal_origins() {
        use crate::articles::params::ParamValue;
        let tmp = tempfile::tempdir().unwrap();
        let prc = tmp.path().join("fighter/samus/param/vl.prc");
        let mut block = ParamBlock { present: true, ..Default::default() };
        block.fields.insert("speed".into(), (ParamValue::Float(1.5), prc));
        let s = setting(
            SettingValue::Param { block: "param_lativolt".into(), field: "speed".into() },
            PathBuf::from("f.rs"),
            0..1,
        );
        let rows = setting_rows(&[s], &block);
        assert_eq!(rows[0].value, "1.5");
        assert_eq!(rows[0].origin, "param_lativolt › speed (samus vl.prc)");
        assert!(rows[0].editable);

        let file = tmp.path().join("status").join("fly.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "fn a() {\n    set(\n 2.0);\n}\n").unwrap();
        let at = "fn a() {\n    set(\n ".len();
        let s = setting(SettingValue::Literal(2.0), file, at..at + 3);
        let rows = setting_rows_in(&[s], &block, SettingContext { root: Some(tmp.path()), ..Default::default() });
        assert_eq!(rows[0].value, "2");
        assert_eq!(rows[0].origin, "status/fly.rs:3");
        assert!(rows[0].editable);
    }

    #[test]
    fn status_site_call_is_read_only_with_file_line() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("status.rs");
        let text = "// header\nunsafe extern \"C\" fn special_n_main(fighter: &mut L2CFighterCommon) -> L2CValue {\n    ArticleModule::generate_article(fighter.module_accessor, *FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL, false, -1);\n    0.into()\n}\n";
        std::fs::write(&file, text).unwrap();
        let start = "// header\n".len();
        let site = StatusSite {
            kind_expr: "*FIGHTER_STATUS_KIND_SPECIAL_N".into(),
            phase: "Main".into(),
            function: "special_n_main".into(),
            file: file.clone(),
            span: start..text.len(),
        };
        let body = text[start..].to_string();
        let rows = status_call_rows(&[(&site, body)], &catalog(vec![vanilla("mario", "fireball")]), Some(tmp.path()));
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].editable);
        assert_eq!(rows[0].label, "Creates the fireball");
        assert_eq!(rows[0].origin, "in status code, `status.rs:3`");
        assert_eq!(rows[0].line, 3);
    }

    #[test]
    fn status_kinds_map_to_moves() {
        assert!(status_matches_move("*FIGHTER_STATUS_KIND_SPECIAL_N", "special_n"));
        assert!(status_matches_move("*FIGHTER_STATUS_KIND_SPECIAL_N", "special_air_n"));
        assert!(status_matches_move("*FIGHTER_SAMUS_STATUS_KIND_SPECIAL_N_C", "special_n_start"));
        assert!(!status_matches_move("*FIGHTER_STATUS_KIND_SPECIAL_N", "special_s"));
        assert!(status_matches_move("*FIGHTER_STATUS_KIND_ATTACK_S4", "attack_s4_s"));
        assert!(!status_matches_move("", "special_n"));
    }

    /// The real Latios moveset, when it is on this machine: samus lists its four clones with
    /// their badges, and `special_n`'s object calls read in plain words.
    #[test]
    fn latios_moveset_objects() {
        let root = Path::new("C:/Users/notja/Documents/Coding/SSBU Modding/Movesets/Latios-Moveset");
        if !root.is_dir() {
            return;
        }
        let index = crate::acmd_src::SourceIndex::build(root).unwrap();
        let names: Vec<String> =
            ["samus", "mario", "luigi", "link", "koopajr", "miiswordsman"].map(String::from).to_vec();
        let empty = tempfile::tempdir().unwrap();
        let catalog = crate::articles::catalog_for("samus", &index, &names, empty.path());
        let rows = object_rows(&catalog);
        let badge_of = |name: &str| rows.iter().find(|r| r.title == name).map(|r| r.badge.clone());
        assert_eq!(badge_of("lativolt").as_deref(), Some("Clone of Mario's fireball"));
        assert_eq!(badge_of("latisnow").as_deref(), Some("Clone of Luigi's fireball"));
        assert_eq!(badge_of("latihado").as_deref(), Some("Clone of Link's sword_beam"));
        assert_eq!(badge_of("latiball").as_deref(), Some("Clone of Koopajr's cannonball"));
        let mut labels = Vec::new();
        let mut moves: Vec<String> = index.fighters["samus"]
            .scripts
            .keys()
            .filter_map(|s| s.strip_prefix("game_").map(String::from))
            .collect();
        moves.sort();
        for script in &moves {
            if let Some(project) = index.script_source("samus", script) {
                for (frame, call) in crate::acmd::parse_acmd_script(&project.body).to_article_calls() {
                    labels.push(format!("{script} f{frame}: {}", timeline_label(&call, &catalog)));
                }
            }
        }
        for row in status_call_rows(&index.status_bodies("samus"), &catalog, Some(root)) {
            labels.push(format!("status {}: {} ({})", row.status, row.label, row.origin));
        }
        eprintln!("{}", labels.join("
"));
        assert!(labels.iter().all(|l| !l.contains("can't name")), "{labels:#?}");
    }
}
