//! Numeric `const`s a moveset defines and then uses as ACMD values.
//!
//! Authors who tune a move a lot pull its numbers out into named constants:
//!
//! ```text
//! const WALK_LW_DAMAGE:       f32 = 4.0;
//! macros::ATTACK(agent, 0, 0, …, WALK_LW_DAMAGE, WALK_LW_ANGLE as u64, …);
//! frame(agent.lua_state_agent, crate::shyguy::pacman::status::THROW_HI_LIFTOFF_FRAME as f32);
//! ```
//!
//! To the parsers those were not numbers, so the hitbox was a raw line and the frame was lost.
//! This module reads the definitions and does two things with them:
//!
//! - [`ProjectConsts::substitute`] shows the value in the editor's copy of a script, so the call
//!   reads as the hitbox it is;
//! - an edit to such a value is written to the **definition**, in place, through
//!   [`ConstScope`] / [`take_pending`] — never by replacing the name in the call, which would
//!   throw away the author's constant.
//!
//! Only consts whose value is a plain number are understood. Float consts are always shown as
//! values, and so are integer consts that are cast (`NAME as f32`) or named like a quantity
//! (`FRYGUY_HITBOX_KBG`). An integer whose name says it is an identifier — a work flag, a status,
//! a hitbox id — keeps its name, which says more than its number does.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::acmd_src::skip_trivia;
use crate::user_files::Rewrite;

/// One `const NAME: TYPE = VALUE;`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstDef {
    pub file: PathBuf,
    /// The module it is in, as the path its file would have: `src/mario/params` for
    /// `src/mario/params.rs`, for `src/mario/params/mod.rs`, and for a `mod params { … }` in
    /// `src/mario/mod.rs`.
    module: PathBuf,
    /// The bytes of the function or block it is local to.
    block: Option<Range<usize>>,
    /// Byte span of VALUE in `file`.
    pub value_span: Range<usize>,
    /// VALUE as written (`4.0`, `-3`, `60`).
    pub value: String,
    pub ty: String,
    /// How many times the name is referenced across the project.
    pub uses: usize,
}

impl ConstDef {
    fn is_float(&self) -> bool {
        matches!(self.ty.as_str(), "f32" | "f64")
    }

    pub fn number(&self) -> Option<f64> {
        numeric_value(&self.value)
    }
}

/// Every numeric const in a project, by name.
#[derive(Debug, Default, Clone)]
pub struct ProjectConsts {
    defs: HashMap<String, Vec<ConstDef>>,
}

/// Where a const's name is written: the file, and the bytes of the function or value around it.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstUse {
    pub file: PathBuf,
    pub span: Range<usize>,
}

/// A value edit that belongs in a const's definition rather than at the call.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstEdit {
    /// The const as the call writes it: `NAME` or `path::NAME`.
    pub path: String,
    pub value: f64,
    pub used: ConstUse,
}

impl ConstEdit {
    pub fn name(&self) -> &str {
        self.path.rsplit("::").next().unwrap_or(&self.path)
    }
}

thread_local! {
    static SCOPE: RefCell<Option<(Arc<ProjectConsts>, ConstUse)>> = const { RefCell::new(None) };
    static PENDING: RefCell<Vec<ConstEdit>> = const { RefCell::new(Vec::new()) };
}

/// While alive, value edits on a const reference are recorded for [`take_pending`] instead of
/// being dropped.
pub struct ConstScope(Option<(Arc<ProjectConsts>, ConstUse)>);

impl Drop for ConstScope {
    fn drop(&mut self) {
        let previous = self.0.take();
        SCOPE.with(|scope| *scope.borrow_mut() = previous);
    }
}

/// `used` is the function being edited.
pub fn scope(consts: Arc<ProjectConsts>, used: ConstUse) -> ConstScope {
    PENDING.with(|pending| pending.borrow_mut().clear());
    let previous = SCOPE.with(|scope| scope.borrow_mut().replace((consts, used)));
    ConstScope(previous)
}

/// The definition edits recorded since the scope opened.
pub fn take_pending() -> Vec<ConstEdit> {
    PENDING.with(|pending| std::mem::take(&mut *pending.borrow_mut()))
}

/// Called by the numeric write-back helpers when the argument they would replace is not a
/// literal. Returns whether `current` named a const of the project in scope — in which case the
/// edit, if it changes anything, has been recorded against that const's definition.
pub(crate) fn record_edit(current: &str, new_value: f64) -> bool {
    SCOPE.with(|scope| {
        let scope = scope.borrow();
        let Some((consts, used)) = scope.as_ref() else {
            return false;
        };
        let Some((path, def)) = consts.reference(current) else {
            return false;
        };
        let Some(old) = def.number() else {
            return false;
        };
        let unchanged = (old - new_value).abs() < 1e-6
            || respell_like_definition(def, new_value).is_ok_and(|value| value == def.value);
        if !unchanged {
            PENDING.with(|pending| {
                pending.borrow_mut().push(ConstEdit {
                    path: path.to_string(),
                    value: new_value,
                    used: used.clone(),
                })
            });
        }
        true
    })
}

/// The value of `text` if it is a reference to a const of the project in scope.
pub(crate) fn resolve_in_scope(text: &str) -> Option<f64> {
    SCOPE.with(|scope| {
        let scope = scope.borrow();
        let (_, def) = scope.as_ref()?.0.reference(text)?;
        def.number()
    })
}

impl ProjectConsts {
    /// Read every numeric const in `sources`.
    pub fn scan(sources: &[(PathBuf, String)]) -> Self {
        let mut defs: HashMap<String, Vec<ConstDef>> = HashMap::new();
        for (file, text) in sources {
            for (name, def) in const_definitions(file, text) {
                defs.entry(name).or_default().push(def);
            }
        }
        let mut uses: HashMap<&str, usize> = HashMap::new();
        for (_, text) in sources {
            for name in identifiers(text) {
                if defs.contains_key(name) {
                    *uses.entry(name).or_default() += 1;
                }
            }
        }
        for (name, defs) in &mut defs {
            // Each definition names itself once.
            let uses = uses.get(name.as_str()).copied().unwrap_or(0);
            let uses = uses.saturating_sub(defs.len());
            defs.iter_mut().for_each(|def| def.uses = uses);
        }
        ProjectConsts { defs }
    }

    /// A const's value: none for a name defined twice with different values, since which one a
    /// call sees depends on where it is called from ([`Self::definition`]).
    pub fn get(&self, name: &str) -> Option<&ConstDef> {
        let (first, rest) = self.defs.get(name)?.split_first()?;
        rest.iter()
            .all(|def| (&def.value, &def.ty) == (&first.value, &first.ty))
            .then_some(first)
    }

    /// The definition `path` names where it is written, or why it cannot be told.
    pub fn definition(&self, path: &str, used: &ConstUse) -> Result<&ConstDef, String> {
        let mut segments: Vec<&str> = path.split("::").map(str::trim).collect();
        let name = segments.pop().unwrap_or(path);
        let defs = self.defs.get(name).map_or(&[][..], Vec::as_slice);
        if let [only] = defs {
            return Ok(only);
        }
        let here = used_module(used);
        let mut found: Vec<&ConstDef>;
        if segments.is_empty() {
            let inside =
                |block: &Range<usize>| block.start <= used.span.end && used.span.start <= block.end;
            let local =
                |def: &&ConstDef| def.file == used.file && def.block.as_ref().is_some_and(inside);
            found = defs.iter().filter(local).collect();
            if found.is_empty() {
                found = module_consts(defs, &here);
            }
            if found.is_empty() {
                for module in imported_modules(&used.file, name) {
                    let module: Vec<&str> = module.iter().map(String::as_str).collect();
                    for def in consts_at(defs, &here, &module) {
                        if !found.contains(&def) {
                            found.push(def);
                        }
                    }
                }
            }
        } else {
            found = consts_at(defs, &here, &segments);
        }
        match (found.as_slice(), defs) {
            ([only], _) => Ok(only),
            (_, []) => Err(format!("{name} is not a number this project defines")),
            _ => {
                let mut files: Vec<String> = defs.iter().map(|def| file_label(&def.file)).collect();
                files.sort();
                files.dedup();
                Err(format!(
                    "{name} is defined in {} places ({}) and it is not clear which one this uses, \
                     so it was not changed",
                    defs.len(),
                    files.join(", ")
                ))
            }
        }
    }

    /// `name`'s definition, when the project has one only.
    pub fn only(&self, name: &str) -> Option<&ConstDef> {
        match self.defs.get(name)?.as_slice() {
            [only] => Some(only),
            _ => None,
        }
    }

    /// `(path, definition)` when `text` is exactly a reference — `NAME`, `path::NAME`, or either
    /// with an `as TYPE` cast.
    pub fn reference<'a>(&'a self, text: &'a str) -> Option<(&'a str, &'a ConstDef)> {
        let text = text.trim();
        let path = match text.split_once(" as ") {
            Some((path, ty)) if is_numeric_type(ty.trim()) => path.trim(),
            Some(_) => return None,
            None => text,
        };
        if !path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
        {
            return None;
        }
        let name = path.rsplit("::").next()?;
        self.get(name).map(|def| (path, def))
    }

    /// `body` with the consts it uses as values spelled as their values.
    pub fn substitute<'a>(&self, body: &'a str) -> Cow<'a, str> {
        if self.defs.is_empty() {
            return Cow::Borrowed(body);
        }
        let bytes = body.as_bytes();
        let mut out = String::new();
        let mut last = 0;
        let mut i = 0;
        while i < body.len() {
            if let Some(next) = skip_trivia(body, i) {
                i = next;
                continue;
            }
            let starts_ident = (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_')
                && (i == 0
                    || !(is_ident(bytes[i - 1]) || bytes[i - 1] == b'.' || bytes[i - 1] == b':'));
            if !starts_ident {
                i += 1;
                continue;
            }
            // A path `a::b::NAME`.
            let mut end = i;
            loop {
                while end < body.len() && is_ident(bytes[end]) {
                    end += 1;
                }
                if body[end..].starts_with("::")
                    && body[end + 2..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                {
                    end += 2;
                    continue;
                }
                break;
            }
            let path = &body[i..end];
            let name = path.rsplit("::").next().unwrap_or(path);
            let Some(def) = self.get(name) else {
                i = end;
                continue;
            };
            // `NAME as f32`.
            let rest = &body[end..];
            let cast = rest
                .strip_prefix(" as ")
                .map(|after| {
                    let len = after
                        .find(|c: char| !c.is_ascii_alphanumeric())
                        .unwrap_or(after.len());
                    &after[..len]
                })
                .filter(|ty| is_numeric_type(ty));
            // Not a use as a value: a definition, a binding, a path into it.
            let before = body[..i].trim_end();
            let declares =
                before.ends_with("const") || before.ends_with("let") || before.ends_with("static");
            if declares || (cast.is_none() && !def.is_float() && names_an_id(name)) {
                i = end;
                continue;
            }
            let replacement = match cast {
                Some("f32" | "f64") => float_text(&def.value),
                Some(_) => integer_text(&def.value),
                None => strip_suffix(&def.value).to_string(),
            };
            let replaced_end = cast.map_or(end, |ty| end + " as ".len() + ty.len());
            out.push_str(&body[last..i]);
            out.push_str(&replacement);
            last = replaced_end;
            i = replaced_end;
        }
        if last == 0 {
            return Cow::Borrowed(body);
        }
        out.push_str(&body[last..]);
        Cow::Owned(out)
    }
}

/// Bytes of a file to replace, and with what.
type Change = (Range<usize>, String);

/// Changes to the files that define consts: what each holds, and what to change in it.
#[derive(Debug, Default)]
pub struct DefinitionEdits(BTreeMap<PathBuf, (String, Vec<Change>)>);

/// A script's new body, to be written along with the definitions it uses.
pub struct ScriptEdit<'a> {
    pub file: &'a Path,
    /// What `file` held when the body was read.
    pub text: &'a str,
    pub span: Range<usize>,
    pub body: String,
}

impl DefinitionEdits {
    /// Every file to write, the script's own included once whatever else changes in it.
    pub fn rewrites(mut self, script: Option<ScriptEdit>) -> anyhow::Result<Vec<Rewrite>> {
        if let Some(mut script) = script {
            let (text, changes) = self
                .0
                .entry(script.file.to_path_buf())
                .or_insert_with(|| (script.text.to_string(), Vec::new()));
            if text != script.text {
                anyhow::bail!(
                    "{} changed on disk while it was being read — try again",
                    script.file.display()
                );
            }
            // A const defined inside the script moves with its new body.
            for (at, value) in std::mem::take(changes) {
                if at.start < script.span.start || at.end > script.span.end {
                    changes.push((at, value));
                    continue;
                }
                let line = text[..at.start].rfind('\n').map_or(0, |n| n + 1);
                let (declares, old) = (&text[line..at.start], &text[line..at.end]);
                if script.body.matches(old).count() != 1 {
                    anyhow::bail!(
                        "`{}` is defined inside the script and could not be found in it again — \
                         nothing was written",
                        old.trim()
                    );
                }
                script.body = script.body.replacen(old, &format!("{declares}{value}"), 1);
            }
            changes.push((script.span, script.body));
        }
        Ok(self
            .0
            .into_iter()
            .map(|(file, (read, mut changes))| {
                changes.sort_by_key(|(at, _)| std::cmp::Reverse(at.start));
                let mut write = read.clone();
                for (at, value) in changes {
                    write.replace_range(at, &value);
                }
                Rewrite::text(&file, &read, write)
            })
            .collect())
    }
}

/// What recorded definition edits come to, before anything is written.
#[derive(Debug, Default)]
pub struct ConstPlan {
    pub edits: DefinitionEdits,
    /// Edits that were not made, and why.
    pub refused: Vec<String>,
    /// One line per const changed, including how many other places change along with this one.
    pub notes: Vec<String>,
}

/// Plan recorded definition edits against the files as they are now.
pub fn plan_edits(consts: &ProjectConsts, edits: &[ConstEdit]) -> ConstPlan {
    let mut plan = ConstPlan::default();
    // One const is one definition, however many calls use it and however they spell its path.
    let mut wanted: Vec<(&str, &ConstDef, Vec<f64>)> = Vec::new();
    for edit in edits {
        match consts.definition(&edit.path, &edit.used) {
            Err(reason) => plan.refused.push(reason),
            Ok(def) => match wanted.iter_mut().find(|(_, known, _)| *known == def) {
                Some((_, _, values)) => values.push(edit.value),
                None => wanted.push((edit.name(), def, vec![edit.value])),
            },
        }
    }
    for (name, def, values) in wanted {
        let spelled = values
            .iter()
            .map(|value| respell_like_definition(def, *value))
            .collect::<Result<Vec<_>, _>>();
        let spelled = match spelled {
            Ok(mut all) => {
                all.dedup();
                all
            }
            Err(reason) => {
                plan.refused
                    .push(format!("{name} {reason}; it was not changed"));
                continue;
            }
        };
        let [spelled] = spelled.as_slice() else {
            plan.refused.push(format!(
                "{name} was given different values in one edit ({}); a const has one value, so \
                 none was written",
                spelled.join(", ")
            ));
            continue;
        };
        let file = def.file.as_path();
        let read = match plan.edits.0.get(file) {
            Some((read, _)) => read.clone(),
            None => std::fs::read_to_string(file).unwrap_or_default(),
        };
        if read.get(def.value_span.clone()) != Some(def.value.as_str()) {
            plan.refused.push(format!(
                "{name} in {} changed on disk since it was read — rescan and try again",
                file.display()
            ));
            continue;
        }
        plan.edits
            .0
            .entry(file.to_path_buf())
            .or_insert_with(|| (read, Vec::new()))
            .1
            .push((def.value_span.clone(), spelled.clone()));
        let defined = consts.defs.get(name).map_or(1, Vec::len);
        let shared = match (def.uses, defined) {
            (uses, 1) if uses > 1 => {
                format!("; it is used in {uses} places, and all of them now use the new value")
            }
            (uses, 2..) if uses > 1 => {
                "; every place that uses this one now uses the new value".to_string()
            }
            _ => String::new(),
        };
        plan.notes.push(format!(
            "{name} is a constant — changed its definition in {} to {spelled}{shared}",
            file_label(file)
        ));
    }
    plan
}

fn file_label(file: &Path) -> String {
    file.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// The module a source file is, as a path: the file without its extension, or its folder for a
/// `mod.rs`, `lib.rs` or `main.rs`.
fn file_module(file: &Path) -> PathBuf {
    match file.file_stem().and_then(|stem| stem.to_str()) {
        Some("mod" | "lib" | "main") => file.parent().unwrap_or(file).to_path_buf(),
        _ => file.with_extension(""),
    }
}

/// The module the code at `used` is in, inline modules included.
fn used_module(used: &ConstUse) -> PathBuf {
    let module = file_module(&used.file);
    let text = std::fs::read_to_string(&used.file).unwrap_or_default();
    let inline = scopes(&text)
        .into_iter()
        .filter(|scope| scope.range.start < used.span.start && used.span.end <= scope.range.end)
        .map_while(|scope| scope.module)
        .collect::<PathBuf>();
    module.join(inline)
}

/// The consts of `defs` that are items of `module` itself.
fn module_consts<'a>(defs: &'a [ConstDef], module: &Path) -> Vec<&'a ConstDef> {
    defs.iter()
        .filter(|def| def.block.is_none() && def.module == module)
        .collect()
}

/// The consts of `defs` that `path` reaches from the module `here` (`super::params`,
/// `crate::mario::params`, `params`).
fn consts_at<'a>(defs: &'a [ConstDef], here: &Path, path: &[&str]) -> Vec<&'a ConstDef> {
    let mut base = here.to_path_buf();
    let mut rest = path;
    let mut relative = false;
    while let Some((first, tail)) = rest.split_first() {
        match *first {
            "self" => {}
            "super" => {
                base.pop();
            }
            _ => break,
        }
        relative = true;
        rest = tail;
    }
    let rooted = rest.first() == Some(&"crate");
    if rooted {
        rest = &rest[1..];
    }
    let tail: PathBuf = rest.iter().collect();
    if !rooted {
        let exact = module_consts(defs, &base.join(&tail));
        if relative || !exact.is_empty() {
            return exact;
        }
    }
    if rest.is_empty() {
        return Vec::new();
    }
    // From the crate root, or through a module imported by name: the one module ending that way.
    defs.iter()
        .filter(|def| def.block.is_none() && def.module.ends_with(&tail))
        .collect()
}

/// The module paths `file` imports `name` from: `use super::params::*`, `use crate::p::{NAME}`.
fn imported_modules(file: &Path, name: &str) -> Vec<Vec<String>> {
    fn flatten(tree: &syn::UseTree, name: &str, at: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        match tree {
            syn::UseTree::Path(path) => {
                at.push(path.ident.to_string());
                flatten(&path.tree, name, at, out);
                at.pop();
            }
            syn::UseTree::Name(item) if item.ident == name => out.push(at.clone()),
            syn::UseTree::Glob(_) => out.push(at.clone()),
            syn::UseTree::Group(group) => {
                for item in &group.items {
                    flatten(item, name, at, out);
                }
            }
            _ => {}
        }
    }
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let Ok(parsed) = syn::parse_file(&text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in &parsed.items {
        if let syn::Item::Use(import) = item {
            flatten(&import.tree, name, &mut Vec::new(), &mut out);
        }
    }
    out
}

/// One `{ … }` of a source file.
struct Scope {
    range: Range<usize>,
    /// The name of the module it is the body of.
    module: Option<String>,
}

/// Every brace pair of `text`, outermost first.
fn scopes(text: &str) -> Vec<Scope> {
    let bytes = text.as_bytes();
    let mut open: Vec<(usize, Option<String>)> = Vec::new();
    let mut out = Vec::new();
    // Where the item now being read starts: after the last `;`, `{` or `}`.
    let mut item = 0;
    let mut i = 0;
    while i < text.len() {
        if let Some(next) = skip_trivia(text, i) {
            i = next;
            continue;
        }
        match bytes[i] {
            b'{' => {
                let mut words = text[item..i].split_whitespace().rev();
                let name = words.next().filter(|_| words.next() == Some("mod"));
                open.push((i, name.map(str::to_string)));
                item = i + 1;
            }
            b'}' => {
                if let Some((start, module)) = open.pop() {
                    out.push(Scope {
                        range: start..i + 1,
                        module,
                    });
                }
                item = i + 1;
            }
            b';' => item = i + 1,
            _ => {}
        }
        i += 1;
    }
    out.sort_by_key(|scope| scope.range.start);
    out
}

fn const_definitions(file: &Path, text: &str) -> Vec<(String, ConstDef)> {
    let bytes = text.as_bytes();
    let mut scopes_here: Option<Vec<Scope>> = None;
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if let Some(next) = skip_trivia(text, i) {
            i = next;
            continue;
        }
        let is_const = text.is_char_boundary(i)
            && text[i..].starts_with("const ")
            && (i == 0 || !(is_ident(bytes[i - 1]) || bytes[i - 1] == b'*'));
        if !is_const {
            i += 1;
            continue;
        }
        i += "const ".len();
        let rest = &text[i..];
        let name_len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(0);
        if name_len == 0 {
            continue;
        }
        let name = &rest[..name_len];
        let Some(after_name) = rest[name_len..].trim_start().strip_prefix(':') else {
            continue;
        };
        let Some((ty, after_ty)) = after_name.split_once('=') else {
            continue;
        };
        let ty = ty.trim();
        if !is_numeric_type(ty) {
            continue;
        }
        let Some(semi) = after_ty.find(';') else {
            continue;
        };
        let raw = &after_ty[..semi];
        let value = raw.trim();
        if numeric_value(value).is_none() {
            continue;
        }
        let value_start = text.len() - after_ty.len() + (raw.len() - raw.trim_start().len());
        let around: Vec<&Scope> = scopes_here
            .get_or_insert_with(|| scopes(text))
            .iter()
            .filter(|scope| scope.range.contains(&value_start))
            .collect();
        out.push((
            name.to_string(),
            ConstDef {
                file: file.to_path_buf(),
                module: file_module(file).join(
                    around
                        .iter()
                        .map_while(|scope| scope.module.clone())
                        .collect::<PathBuf>(),
                ),
                block: around
                    .iter()
                    .rfind(|scope| scope.module.is_none())
                    .map(|scope| scope.range.clone()),
                value_span: value_start..value_start + value.len(),
                value: value.to_string(),
                ty: ty.to_string(),
                uses: 0,
            },
        ));
        i = value_start + value.len();
    }
    out
}

fn identifiers(text: &str) -> impl Iterator<Item = &str> {
    let bytes = text.as_bytes();
    let mut i = 0;
    std::iter::from_fn(move || {
        while i < text.len() {
            if let Some(next) = skip_trivia(text, i) {
                i = next;
                continue;
            }
            if (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_')
                && (i == 0 || !is_ident(bytes[i - 1]))
            {
                let start = i;
                while i < text.len() && is_ident(bytes[i]) {
                    i += 1;
                }
                return Some(&text[start..i]);
            }
            i += 1;
        }
        None
    })
}

/// Whether an integer const's name says it identifies something (a work slot, a status, a
/// hitbox id) rather than measuring something. Those keep their name on screen even when used
/// bare; a bare `FRYGUY_HITBOX_KBG` is a quantity and shows its value.
fn names_an_id(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper == "ID"
        || upper.ends_with("_ID")
        || upper.starts_with("ID_")
        || upper.contains("_ID_")
        || [
            "FLAG", "WORK", "KIND", "STATUS", "TERM", "INSTANCE", "ARTICLE", "GENERATE", "SLOT",
            "INDEX",
        ]
        .iter()
        .any(|word| upper.contains(word))
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_numeric_type(ty: &str) -> bool {
    matches!(
        ty,
        "f32"
            | "f64"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "usize"
            | "isize"
    )
}

const SUFFIXES: [&str; 12] = [
    "f32", "f64", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "usize", "isize",
];

fn strip_suffix(value: &str) -> &str {
    SUFFIXES
        .iter()
        .find_map(|suffix| value.strip_suffix(suffix))
        .map(|v| v.trim_end_matches('_'))
        .unwrap_or(value)
}

fn numeric_value(value: &str) -> Option<f64> {
    let plain = strip_suffix(value.trim()).replace('_', "");
    let digits = plain.strip_prefix('-').unwrap_or(&plain);
    if digits.is_empty() || !digits.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    plain.parse::<f64>().ok()
}

fn float_text(value: &str) -> String {
    let plain = strip_suffix(value).replace('_', "");
    if plain.contains('.') || plain.contains('e') {
        plain
    } else {
        format!("{plain}.0")
    }
}

fn integer_text(value: &str) -> String {
    numeric_value(value).map_or_else(|| value.to_string(), |v| format!("{}", v.trunc() as i64))
}

/// `new` written as a decimal number, with the suffix `existing` has.
fn format_float_like(existing: &str, new: f64) -> String {
    let suffix = SUFFIXES
        .iter()
        .find(|s| existing.ends_with(**s))
        .copied()
        .unwrap_or("");
    let mut text = format!("{:.4}", new);
    while text.ends_with('0') && !text.ends_with(".0") {
        text.pop();
    }
    format!("{text}{suffix}")
}

/// A number as a message says it: `4.3`, not the digits an `f32` adds when widened.
fn shown(value: f64) -> String {
    let single = value as f32;
    if single.is_finite() && (f64::from(single) - value).abs() < 1e-6 * value.abs().max(1.0) {
        single.to_string()
    } else {
        value.to_string()
    }
}

/// `value` spelled like `def`'s value, or why its type cannot hold it: a whole-number const is
/// never rounded to fit.
fn respell_like_definition(def: &ConstDef, value: f64) -> Result<String, String> {
    if !value.is_finite() {
        return Err(format!("can't hold {}", shown(value)));
    }
    if def.is_float() {
        return Ok(format_float_like(&def.value, value));
    }
    if value.fract() != 0.0 {
        return Err(format!(
            "is a whole-number constant, so it can't hold {}",
            shown(value)
        ));
    }
    let whole = format!("{value:.0}");
    let fits = match def.ty.as_str() {
        "i8" => whole.parse::<i8>().is_ok(),
        "i16" => whole.parse::<i16>().is_ok(),
        "i32" => whole.parse::<i32>().is_ok(),
        "u8" => whole.parse::<u8>().is_ok(),
        "u16" => whole.parse::<u16>().is_ok(),
        "u32" => whole.parse::<u32>().is_ok(),
        "u64" | "usize" => whole.parse::<u64>().is_ok(),
        _ => whole.parse::<i64>().is_ok(),
    };
    if !fits {
        return Err(format!(
            "is a {}, so it can't hold {}",
            def.ty,
            shown(value)
        ));
    }
    format_literal_like(&def.value, value).ok_or_else(|| {
        format!(
            "is written as `{}`, so it can't hold {}",
            def.value,
            shown(value)
        )
    })
}

/// `new` written in the style of the numeric literal `existing`: `30` gives `45`, `30.0` gives
/// `45.0`, `1.6f32` gives `2.0f32`. A literal written as a whole number (no decimal point and no
/// float suffix) is `None` for a value with a fraction, which it cannot hold.
pub(crate) fn format_literal_like(existing: &str, new: f64) -> Option<String> {
    let existing = existing.trim();
    if !new.is_finite() {
        return None;
    }
    if let Some(radix_literal) = format_radix_like(existing, new) {
        return radix_literal;
    }
    let plain = strip_suffix(existing);
    let has_point = plain.contains('.') || plain.contains(['e', 'E']);
    let float_suffix = existing.ends_with("f32") || existing.ends_with("f64");
    if has_point {
        return Some(format_float_like(existing, new));
    }
    if new.fract() != 0.0 {
        // `30f32` holds a fraction; a bare `30` or `30u8` does not.
        return float_suffix.then(|| format_float_like(existing, new));
    }
    let suffix = &existing[plain.len()..];
    Some(format!("{}{suffix}", new as i64))
}

/// For a `0x`/`0o`/`0b` integer literal, `new` in the same base, letter case and integer suffix
/// (`0x1E` → `0x2D`, `0x1eu8` → `0x2du8`), or `Some(None)` when `new` has a fraction. `None` when
/// `existing` is not such a literal. In these bases `e`, `E` and `f32` are digits, not an exponent
/// or a float suffix. (Project consts are only read in decimal, so none is ever one of these.)
fn format_radix_like(existing: &str, new: f64) -> Option<Option<String>> {
    let body = existing.strip_prefix('-').unwrap_or(existing);
    let prefix = body.get(..2)?;
    let radix = match prefix {
        "0x" | "0X" => 16,
        "0o" => 8,
        "0b" => 2,
        _ => return None,
    };
    if new.fract() != 0.0 {
        return Some(None);
    }
    let rest = &body[2..];
    let suffix = [
        "u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize",
    ]
    .into_iter()
    .find(|suffix| rest.ends_with(suffix))
    .unwrap_or("");
    let digits = &rest[..rest.len() - suffix.len()];
    let separator = if digits.ends_with('_') { "_" } else { "" };
    let magnitude = new.abs() as u64;
    let value = match radix {
        16 if digits.chars().any(|c| c.is_ascii_uppercase()) => format!("{magnitude:X}"),
        16 => format!("{magnitude:x}"),
        8 => format!("{magnitude:o}"),
        _ => format!("{magnitude:b}"),
    };
    let sign = if new < 0.0 { "-" } else { "" };
    Some(Some(format!("{sign}{prefix}{value}{separator}{suffix}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R11a: a literal keeps its own spelling; in hex, `e` and `f32` are digits.
    #[test]
    fn literals_are_respelled_in_their_own_style() {
        let cases: [(&str, f64, Option<&str>); 12] = [
            ("30", 45.0, Some("45")),
            ("30.0", 45.0, Some("45.0")),
            ("1.6f32", 2.0, Some("2.0f32")),
            ("30f32", 45.5, Some("45.5f32")),
            ("30", 45.5, None),
            ("1e3", 45.0, Some("45.0")),
            ("0x1E", 45.0, Some("0x2D")),
            ("0x1e", 45.0, Some("0x2d")),
            ("0x1Eu8", 45.0, Some("0x2Du8")),
            ("0x1f32", 45.0, Some("0x2d")),
            ("0b101", 6.0, Some("0b110")),
            ("0x1E", 45.5, None),
        ];
        for (existing, new, expected) in cases {
            assert_eq!(
                format_literal_like(existing, new).as_deref(),
                expected,
                "{existing} -> {new}"
            );
        }
        // Project consts are read in decimal only, so a hex const is never an edit target.
        let hex =
            ProjectConsts::scan(&[(PathBuf::from("c.rs"), "const X: i32 = 0x1E;\n".to_string())]);
        assert!(hex.get("X").is_none());
    }

    const SPECIALS: &str = "const WALK_LW_DAMAGE:       f32 = 4.0;\nconst WALK_LW_ANGLE:        i32 = 70;\nconst HITBOX_ID: u64 = 2;\npub(crate) const LIFTOFF: i32 = 6;\nlet p: *const u8 = 0 as *const u8;\n// const COMMENTED: f32 = 9.0;\nfn game_x(agent: &mut L2CAgentBase) {\n    frame(agent.lua_state_agent, crate::a::LIFTOFF as f32);\n    macros::ATTACK(agent, HITBOX_ID, 0, Hash40::new(\"top\"), WALK_LW_DAMAGE, WALK_LW_ANGLE as u64, 1);\n}\n";

    fn consts() -> ProjectConsts {
        ProjectConsts::scan(&[(PathBuf::from("specials.rs"), SPECIALS.to_string())])
    }

    #[test]
    fn numeric_consts_are_read_with_their_value_span() {
        let consts = consts();
        let damage = consts.get("WALK_LW_DAMAGE").unwrap();
        assert_eq!(damage.value, "4.0");
        assert_eq!(&SPECIALS[damage.value_span.clone()], "4.0");
        assert_eq!(damage.uses, 1);
        assert!(consts.get("COMMENTED").is_none());
        assert!(consts.get("p").is_none());
        assert_eq!(consts.get("LIFTOFF").unwrap().value, "6");
    }

    #[test]
    fn values_show_where_they_are_used_as_values_and_ids_keep_their_names() {
        let body = consts().substitute(SPECIALS).into_owned();
        assert!(
            body.contains("frame(agent.lua_state_agent, 6.0);"),
            "{body}"
        );
        assert!(
            body.contains("macros::ATTACK(agent, HITBOX_ID, 0, Hash40::new(\"top\"), 4.0, 70, 1);"),
            "{body}"
        );
        assert!(
            body.contains("const WALK_LW_DAMAGE:       f32 = 4.0;"),
            "{body}"
        );
    }

    #[test]
    fn an_edit_on_a_const_is_recorded_against_its_definition() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("specials.rs");
        std::fs::write(&file, SPECIALS).unwrap();
        let consts = Arc::new(ProjectConsts::scan(&[(file.clone(), SPECIALS.to_string())]));
        let used = ConstUse {
            file: file.clone(),
            span: SPECIALS.find("fn game_x").unwrap()..SPECIALS.len(),
        };
        let edits = {
            let _scope = scope(Arc::clone(&consts), used.clone());
            assert!(record_edit("WALK_LW_DAMAGE", 5.5));
            assert!(
                record_edit("WALK_LW_ANGLE as u64", 70.0),
                "unchanged: recognised, nothing recorded"
            );
            assert!(!record_edit("other", 1.0));
            take_pending()
        };
        assert_eq!(
            edits,
            [ConstEdit {
                path: "WALK_LW_DAMAGE".into(),
                value: 5.5,
                used,
            }]
        );
        assert!(
            !record_edit("WALK_LW_DAMAGE", 1.0),
            "no scope, no recording"
        );
        let plan = plan_edits(&consts, &edits);
        assert_eq!((plan.notes.len(), plan.refused.len()), (1, 0));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            SPECIALS,
            "planned, not written"
        );
        let rewrites = plan.edits.rewrites(None).unwrap();
        crate::user_files::write_all(&rewrites).unwrap();
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(
            written.starts_with("const WALK_LW_DAMAGE:       f32 = 5.5;\n"),
            "{written}"
        );
        assert_eq!(written.len(), SPECIALS.len(), "only the value changed");
    }

    /// Dedede and Mario both keep an `INIT_DAMAGE` in `status::params`, as inline modules of one
    /// file: each path reaches its own, and a value its type cannot hold is refused.
    #[test]
    fn a_path_reaches_the_module_it_names() {
        let text = "pub mod dedede {\n    pub mod params { pub const INIT_DAMAGE: u8 = 4; }\n}\npub mod mario {\n    pub mod params { pub const INIT_DAMAGE: u8 = 4; }\n    fn status() { super::dedede::params::INIT_DAMAGE; params::INIT_DAMAGE; }\n}\n";
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("src/status.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, text).unwrap();
        let consts = ProjectConsts::scan(&[(file.clone(), text.to_string())]);
        let start = text.find("fn status").unwrap();
        let used = ConstUse {
            file,
            span: start..start + text[start..].find('}').unwrap(),
        };
        let first = text.find("= 4").unwrap() + 2;
        let second = text.rfind("= 4").unwrap() + 2;
        for (path, at) in [
            ("params::INIT_DAMAGE", Some(second)),
            ("self::params::INIT_DAMAGE", Some(second)),
            ("super::dedede::params::INIT_DAMAGE", Some(first)),
            ("crate::status::dedede::params::INIT_DAMAGE", Some(first)),
            ("crate::params::INIT_DAMAGE", None),
            ("INIT_DAMAGE", None),
        ] {
            let found = consts.definition(path, &used).ok();
            assert_eq!(found.map(|def| def.value_span.start), at, "{path}");
        }
        let edit = |value: f64| ConstEdit {
            path: "params::INIT_DAMAGE".into(),
            value,
            used: used.clone(),
        };
        for value in [4.5, 256.0, -1.0, f64::NAN] {
            let plan = plan_edits(&consts, &[edit(value)]);
            assert_eq!((plan.refused.len(), plan.notes.len()), (1, 0), "{value}");
            assert!(plan.edits.rewrites(None).unwrap().is_empty());
        }
        let plan = plan_edits(&consts, &[edit(9.0), edit(9.0)]);
        let rewrites = plan.edits.rewrites(None).unwrap();
        let written = String::from_utf8(rewrites[0].write.clone()).unwrap();
        assert_eq!(written.matches("= 9;").count(), 1);
        assert!(written.rfind("= 9;") > written.find("= 4;"), "{written}");
        assert_eq!(
            plan_edits(&consts, &[edit(8.0), edit(9.0)]).refused.len(),
            1
        );
    }
}
