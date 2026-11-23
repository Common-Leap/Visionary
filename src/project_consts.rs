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
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One `const NAME: TYPE = VALUE;`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstDef {
    pub file: PathBuf,
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

/// Every resolvable numeric const in a project, by name.
#[derive(Debug, Default, Clone)]
pub struct ProjectConsts {
    defs: HashMap<String, ConstDef>,
}

/// A value edit that belongs in a const's definition rather than at the call.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstEdit {
    pub name: String,
    pub value: String,
}

thread_local! {
    static SCOPE: RefCell<Option<Arc<ProjectConsts>>> = const { RefCell::new(None) };
    static PENDING: RefCell<Vec<ConstEdit>> = const { RefCell::new(Vec::new()) };
}

/// While alive, value edits on a const reference are recorded for [`take_pending`] instead of
/// being dropped.
pub struct ConstScope(Option<Arc<ProjectConsts>>);

impl Drop for ConstScope {
    fn drop(&mut self) {
        let previous = self.0.take();
        SCOPE.with(|scope| *scope.borrow_mut() = previous);
    }
}

pub fn scope(consts: Arc<ProjectConsts>) -> ConstScope {
    PENDING.with(|pending| pending.borrow_mut().clear());
    let previous = SCOPE.with(|scope| scope.borrow_mut().replace(consts));
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
        let Some(consts) = scope.as_ref() else {
            return false;
        };
        let Some((name, def)) = consts.reference(current) else {
            return false;
        };
        let Some(old) = def.number() else {
            return false;
        };
        let value = format_like(&def.value, def.is_float(), new_value);
        if (old - new_value).abs() < 1e-6 || value == def.value {
            return true;
        }
        PENDING.with(|pending| {
            pending.borrow_mut().push(ConstEdit {
                name: name.to_string(),
                value,
            })
        });
        true
    })
}

/// The value of `text` if it is a reference to a const of the project in scope.
pub(crate) fn resolve_in_scope(text: &str) -> Option<f64> {
    SCOPE.with(|scope| {
        let scope = scope.borrow();
        let (_, def) = scope.as_ref()?.reference(text)?;
        def.number()
    })
}

impl ProjectConsts {
    /// Read every numeric const in `sources`. A name defined twice with different values is
    /// dropped: which one a call sees depends on module scoping this does not model.
    pub fn scan(sources: &[(PathBuf, String)]) -> Self {
        let mut found: HashMap<String, Option<ConstDef>> = HashMap::new();
        for (file, text) in sources {
            for (name, def) in const_definitions(file, text) {
                found
                    .entry(name)
                    .and_modify(|known| {
                        if known.as_ref().map(|k| (&k.value, &k.ty)) != Some((&def.value, &def.ty))
                        {
                            *known = None;
                        }
                    })
                    .or_insert(Some(def));
            }
        }
        let mut defs: HashMap<String, ConstDef> = found
            .into_iter()
            .filter_map(|(name, def)| Some((name, def?)))
            .collect();
        for (_, text) in sources {
            for name in identifiers(text) {
                if let Some(def) = defs.get_mut(name) {
                    def.uses += 1;
                }
            }
        }
        // The definition names itself once.
        for def in defs.values_mut() {
            def.uses = def.uses.saturating_sub(1);
        }
        ProjectConsts { defs }
    }

    pub fn get(&self, name: &str) -> Option<&ConstDef> {
        self.defs.get(name)
    }

    /// `(name, definition)` when `text` is exactly a reference — `NAME`, `path::NAME`, or either
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
        self.defs.get(name).map(|def| (name, def))
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
            if let Some(next) = skip_literal_or_comment(body, i) {
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
            let Some(def) = self.defs.get(name) else {
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

/// Apply recorded definition edits to disk. Returns the files written and one note per const —
/// including how many other places a shared const changes along with this one.
pub fn apply_edits(
    consts: &ProjectConsts,
    edits: &[ConstEdit],
) -> (Vec<PathBuf>, Vec<String>, usize) {
    let mut notes = Vec::new();
    let mut by_name: Vec<(&str, &str)> = Vec::new();
    for edit in edits {
        match by_name.iter().find(|(name, _)| *name == edit.name) {
            Some((_, value)) if *value != edit.value => {
                notes.push(format!(
                    "{} was given two different values in one edit ({} and {}); a const has one \
                     value, so neither was written",
                    edit.name, value, edit.value
                ));
                by_name.retain(|(name, _)| *name != edit.name);
            }
            Some(_) => {}
            None => by_name.push((&edit.name, &edit.value)),
        }
    }
    let mut per_file: HashMap<&Path, Vec<(&str, &ConstDef, &str)>> = HashMap::new();
    for (name, value) in &by_name {
        if let Some(def) = consts.get(name) {
            per_file
                .entry(def.file.as_path())
                .or_default()
                .push((name, def, value));
        }
    }
    let mut files = Vec::new();
    let mut changed = 0;
    for (file, mut edits) in per_file {
        let Ok(mut text) = std::fs::read_to_string(file) else {
            notes.push(format!(
                "could not read {} to change its consts",
                file.display()
            ));
            continue;
        };
        edits.sort_by_key(|(_, def, _)| std::cmp::Reverse(def.value_span.start));
        let mut wrote = false;
        for (name, def, value) in edits {
            if text.get(def.value_span.clone()) != Some(def.value.as_str()) {
                notes.push(format!(
                    "{name} in {} changed on disk since it was read — rescan and try again",
                    file.display()
                ));
                continue;
            }
            // An edit may carry a plain number (`plan_setting_write` has no definition to spell
            // it like): it is written the way the definition is, and an integer const refuses a
            // fraction rather than rounding it.
            let Some(value) = respell_like_definition(def, value) else {
                notes.push(format!(
                    "{name} is a whole-number constant, so it can't hold {value}; it was not changed"
                ));
                continue;
            };
            let value = value.as_str();
            text.replace_range(def.value_span.clone(), value);
            wrote = true;
            changed += 1;
            let shared = if def.uses > 1 {
                format!(
                    "; it is used in {} places, and all of them now use the new value",
                    def.uses
                )
            } else {
                String::new()
            };
            notes.push(format!(
                "{name} is a constant — changed its definition in {} to {value}{shared}",
                file.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
        if wrote {
            if std::fs::write(file, &text).is_ok() {
                files.push(file.to_path_buf());
            } else {
                notes.push(format!("could not write {}", file.display()));
            }
        }
    }
    (files, notes, changed)
}

fn const_definitions(file: &Path, text: &str) -> Vec<(String, ConstDef)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if let Some(next) = skip_literal_or_comment(text, i) {
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
        out.push((
            name.to_string(),
            ConstDef {
                file: file.to_path_buf(),
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
            if let Some(next) = skip_literal_or_comment(text, i) {
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

/// `new` written the way `existing` is: same suffix, a decimal point when it had one.
fn format_like(existing: &str, float: bool, new: f64) -> String {
    let suffix = SUFFIXES
        .iter()
        .find(|s| existing.ends_with(**s))
        .copied()
        .unwrap_or("");
    let body = if float || existing.contains('.') {
        let mut text = format!("{:.4}", new);
        while text.ends_with('0') && !text.ends_with(".0") {
            text.pop();
        }
        text
    } else {
        format!("{}", new.round() as i64)
    };
    format!("{body}{suffix}")
}

/// `value` spelled like `def`'s value. A value that is not a plain number is kept as given; a
/// fraction for an integer const is `None`.
fn respell_like_definition(def: &ConstDef, value: &str) -> Option<String> {
    let Some(number) = numeric_value(value) else {
        return Some(value.to_string());
    };
    if def.is_float() {
        return Some(format_like(&def.value, true, number));
    }
    format_literal_like(&def.value, number)
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
        return Some(format_like(existing, true, new));
    }
    if new.fract() != 0.0 {
        // `30f32` holds a fraction; a bare `30` or `30u8` does not.
        return float_suffix.then(|| format_like(existing, true, new));
    }
    let suffix = &existing[plain.len()..];
    Some(format!("{}{suffix}", new as i64))
}

/// For a `0x`/`0o`/`0b` integer literal, `new` in the same base, letter case and integer suffix
/// (`0x1E` → `0x2D`, `0x1eu8` → `0x2du8`), or `Some(None)` when `new` has a fraction. `None` when
/// `existing` is not such a literal. In these bases `e`, `E` and `f32` are digits, not an exponent
/// or a float suffix. (Project consts are only read in decimal, so `format_like` never meets one.)
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

fn skip_literal_or_comment(text: &str, i: usize) -> Option<usize> {
    if !text.is_char_boundary(i) {
        return None;
    }
    let rest = &text[i..];
    if let Some(body) = rest.strip_prefix("//") {
        return Some(i + 2 + body.find('\n').map_or(body.len(), |n| n + 1));
    }
    if let Some(body) = rest.strip_prefix("/*") {
        return Some(body.find("*/").map_or(text.len(), |end| i + 4 + end));
    }
    if rest.starts_with('"') {
        let mut escaped = false;
        for (offset, ch) in rest.char_indices().skip(1) {
            match ch {
                '\\' if !escaped => escaped = true,
                '"' if !escaped => return Some(i + offset + 1),
                _ => escaped = false,
            }
        }
        return Some(text.len());
    }
    None
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
        let consts = Arc::new(consts());
        {
            let _scope = scope(Arc::clone(&consts));
            assert!(record_edit("WALK_LW_DAMAGE", 5.5));
            assert!(
                record_edit("WALK_LW_ANGLE as u64", 70.0),
                "unchanged: recognised, nothing recorded"
            );
            assert!(!record_edit("other", 1.0));
            assert_eq!(
                take_pending(),
                [ConstEdit {
                    name: "WALK_LW_DAMAGE".into(),
                    value: "5.5".into()
                }]
            );
        }
        assert!(
            !record_edit("WALK_LW_DAMAGE", 1.0),
            "no scope, no recording"
        );
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("specials.rs");
        std::fs::write(&file, SPECIALS).unwrap();
        let consts = ProjectConsts::scan(&[(file.clone(), SPECIALS.to_string())]);
        let (files, notes, changed) = apply_edits(
            &consts,
            &[ConstEdit {
                name: "WALK_LW_DAMAGE".into(),
                value: "5.5".into(),
            }],
        );
        assert_eq!((files.len(), changed), (1, 1), "{notes:?}");
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(
            written.starts_with("const WALK_LW_DAMAGE:       f32 = 5.5;\n"),
            "{written}"
        );
        assert_eq!(written.len(), SPECIALS.len(), "only the value changed");
    }
}
