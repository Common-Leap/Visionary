//! A project's own `Hash40` shorthand macros: `cmn!("swing_06")`, `se!("jump01")`, `vc!("ottotto")`.
//!
//! Movesets rarely spell every sound out as `Hash40::new("se_common_swing_06")`. Shy Guy, for
//! one, writes `cmn!`, `se!`, `vc!` and `rb!`, each a one-line `macro_rules!` around
//! `Hash40::new(concat!(…))` — some through a second macro (`vc!` → `fighter_vc!("pacman", …)`).
//! To the editor those calls were opaque, so every sound written that way was invisible, and
//! worse, still counted as a call site, which shifted later sound edits onto the wrong line.
//!
//! This module reads those definitions out of the linked project and does two things with them:
//!
//! - [`HashMacros::expand_in`] spells each resolvable call out as `Hash40::new("…")` in the copy
//!   of a script the editor reads, so the existing parsers see ordinary names;
//! - [`HashMacros::spell`] writes an edited name back in the form the author used: the same macro
//!   with a new literal when the name still fits it, another of the project's macros when that
//!   one fits instead (`se!` → `cmn!`), and `Hash40::new("…")` only when none does.
//!
//! Only literal-in, literal-out macros are understood. Anything else (a macro taking an `expr`,
//! a `stringify!`, more than one arm) is left exactly as written and stays a raw line.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

/// One piece of a macro's `concat!`: fixed text, or the call's n-th argument.
#[derive(Debug, Clone, PartialEq)]
enum Part {
    Lit(String),
    Param(usize),
}

/// What a macro arm expands to, before its own macros are resolved.
#[derive(Debug, Clone, PartialEq)]
enum Body {
    /// `Hash40::new(concat!(…))`, `Hash40::new($x)` or `Hash40::new("…")`.
    Hash(Vec<Part>),
    /// `$crate::other!(…)`: another macro, with these arguments.
    Nested { name: String, args: Vec<Part> },
}

#[derive(Debug, Clone, PartialEq)]
struct Definition {
    params: usize,
    body: Body,
}

/// Every resolvable hash macro in one project, by name.
#[derive(Debug, Default, Clone)]
pub struct HashMacros {
    templates: HashMap<String, (usize, Vec<Part>)>,
}

static CURRENT: LazyLock<RwLock<Arc<HashMacros>>> =
    LazyLock::new(|| RwLock::new(Arc::new(HashMacros::default())));

thread_local! {
    static SCOPED: std::cell::RefCell<Option<Arc<HashMacros>>> = const { std::cell::RefCell::new(None) };
}

/// While alive, [`current`] on this thread answers with one project's macros.
///
/// A sync writes into one project, and must spell edits with *that* project's macros whatever
/// was indexed last process-wide — another project linked meanwhile, or, in the test suite,
/// another test's index built on a parallel thread.
pub struct MacroScope(Option<Arc<HashMacros>>);

impl Drop for MacroScope {
    fn drop(&mut self) {
        let previous = self.0.take();
        SCOPED.with(|scoped| *scoped.borrow_mut() = previous);
    }
}

pub fn scope(macros: Arc<HashMacros>) -> MacroScope {
    let previous = SCOPED.with(|scoped| scoped.borrow_mut().replace(macros));
    MacroScope(previous)
}

/// The macros of the project being written in this thread's [`scope`], else of the project
/// most recently indexed, for write-back paths that only see text.
pub fn current() -> Arc<HashMacros> {
    if let Some(scoped) = SCOPED.with(|scoped| scoped.borrow().clone()) {
        return scoped;
    }
    CURRENT
        .read()
        .map(|guard| Arc::clone(&guard))
        .unwrap_or_default()
}

pub fn set_current(macros: HashMacros) {
    if let Ok(mut guard) = CURRENT.write() {
        *guard = Arc::new(macros);
    }
}

impl HashMacros {
    /// Read every `macro_rules!` in `sources` and keep the ones that resolve to a hash.
    ///
    /// A name defined twice with different bodies is dropped: which one a call site gets
    /// depends on module scoping this reader does not model.
    pub fn scan<'a>(sources: impl IntoIterator<Item = &'a str>) -> Self {
        let mut definitions: HashMap<String, Option<Definition>> = HashMap::new();
        for text in sources {
            for (name, definition) in macro_definitions(text) {
                definitions
                    .entry(name)
                    .and_modify(|known| {
                        if known.as_ref() != Some(&definition) {
                            *known = None;
                        }
                    })
                    .or_insert(Some(definition));
            }
        }
        let definitions: HashMap<String, Definition> = definitions
            .into_iter()
            .filter_map(|(name, definition)| Some((name, definition?)))
            .collect();
        let mut templates = HashMap::new();
        for (name, definition) in &definitions {
            if let Some(parts) = resolve(&definitions, &definition.body, 0) {
                templates.insert(name.clone(), (definition.params, parts));
            }
        }
        HashMacros { templates }
    }

    /// The name `call` (`cmn!("swing_06")`, `crate::se!("x")`) hashes, if it is one of ours.
    pub fn expand_call(&self, call: &str) -> Option<String> {
        let (name, args) = parse_call(call.trim())?;
        let (params, parts) = self.templates.get(name)?;
        if args.len() != *params {
            return None;
        }
        let mut out = String::new();
        for part in parts {
            match part {
                Part::Lit(text) => out.push_str(text),
                Part::Param(index) => out.push_str(args.get(*index)?),
            }
        }
        Some(out)
    }

    /// `body` with every resolvable call spelled out as `Hash40::new("…")`.
    pub fn expand_in<'a>(&self, body: &'a str) -> Cow<'a, str> {
        if self.templates.is_empty() || !body.contains('!') {
            return Cow::Borrowed(body);
        }
        let mut out = String::with_capacity(body.len());
        let mut last = 0;
        let mut i = 0;
        let bytes = body.as_bytes();
        while i < body.len() {
            if let Some(next) = skip_literal_or_comment(body, i) {
                i = next;
                continue;
            }
            if bytes[i] == b'!' {
                // Walk back over the macro's path to its start.
                let mut start = i;
                while start > 0
                    && (bytes[start - 1].is_ascii_alphanumeric()
                        || matches!(bytes[start - 1], b'_' | b':' | b'$'))
                {
                    start -= 1;
                }
                let opens = matches!(bytes.get(i + 1), Some(b'(' | b'[' | b'{'));
                if start < i && start >= last && opens {
                    if let Some(close) = call_end(body, i + 2) {
                        if let Some(name) = self.expand_call(&body[start..=close]) {
                            out.push_str(&body[last..start]);
                            out.push_str(&format!("Hash40::new(\"{name}\")"));
                            last = close + 1;
                            i = close + 1;
                            continue;
                        }
                    }
                }
            }
            i += 1;
        }
        if last == 0 {
            return Cow::Borrowed(body);
        }
        out.push_str(&body[last..]);
        Cow::Owned(out)
    }

    /// The text to put in a hash argument currently written as `current` so it names `name`,
    /// keeping the author's spelling wherever one of their macros can say it.
    ///
    /// `context` is the surrounding source: when more than one macro fits, the one this file
    /// already uses wins.
    pub fn spell(&self, current: &str, name: &str, context: &str) -> String {
        if let Some((macro_name, args)) = parse_call(current.trim()) {
            if let Some(spelled) = self.spell_with(macro_name, &args, name) {
                // Keep a written path (`crate::se!`) as written.
                let trimmed = current.trim();
                let path = &trimmed[..trimmed.find(macro_name).unwrap_or(0)];
                return format!("{path}{spelled}");
            }
        } else {
            // Written as a plain hash: keep it one, path and all (`smash::phx::Hash40::new`).
            let trimmed = current.trim();
            let path = trimmed
                .find("Hash40::new(")
                .map_or("", |at| &trimmed[..at])
                .to_string();
            let path = if path
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
            {
                path
            } else {
                String::new()
            };
            return format!("{path}Hash40::new(\"{name}\")");
        }
        let mut candidates: Vec<(&String, String)> = self
            .templates
            .keys()
            .filter_map(|macro_name| {
                let (params, _) = &self.templates[macro_name];
                (*params == 1)
                    .then(|| self.spell_with(macro_name, &[""], name))
                    .flatten()
                    .map(|spelled| (macro_name, spelled))
            })
            .collect();
        // Prefer a macro the file already uses, then the tightest fit (longest fixed text).
        candidates.sort_by_key(|(macro_name, spelled)| {
            (
                !context.contains(&format!("{macro_name}!(")),
                std::cmp::Reverse(name.len().saturating_sub(spelled.len())),
                (*macro_name).clone(),
            )
        });
        match candidates.into_iter().next() {
            Some((_, spelled)) => spelled,
            None => format!("Hash40::new(\"{name}\")"),
        }
    }

    /// `macro_name!(…)` with its LAST argument chosen so the call hashes `name`, keeping the
    /// other arguments; `None` when the fixed text around that argument does not fit.
    fn spell_with(&self, macro_name: &str, args: &[&str], name: &str) -> Option<String> {
        let (params, parts) = self.templates.get(macro_name)?;
        if args.len() != *params || *params == 0 {
            return None;
        }
        let free = params - 1;
        // Everything but the free argument is fixed text once the others are substituted.
        let mut prefix = String::new();
        let mut suffix = String::new();
        let mut seen_free = false;
        for part in parts {
            let text = match part {
                Part::Lit(text) => text.as_str(),
                Part::Param(index) if *index == free => {
                    if seen_free {
                        return None; // used twice: no single value to solve for
                    }
                    seen_free = true;
                    continue;
                }
                Part::Param(index) => args.get(*index)?,
            };
            if seen_free {
                suffix.push_str(text);
            } else {
                prefix.push_str(text);
            }
        }
        if !seen_free || name.len() < prefix.len() + suffix.len() {
            return None;
        }
        let value = name.strip_prefix(&prefix)?.strip_suffix(&suffix)?;
        if value.is_empty() || value.contains(['"', '\\']) {
            return None;
        }
        let mut rendered: Vec<String> = args.iter().map(|arg| format!("\"{arg}\"")).collect();
        rendered[free] = format!("\"{value}\"");
        Some(format!("{macro_name}!({})", rendered.join(", ")))
    }
}

/// Resolve a definition's body to fixed text and its own parameters.
fn resolve(
    definitions: &HashMap<String, Definition>,
    body: &Body,
    depth: usize,
) -> Option<Vec<Part>> {
    if depth > 8 {
        return None;
    }
    match body {
        Body::Hash(parts) => Some(parts.clone()),
        Body::Nested { name, args } => {
            let inner = definitions.get(name)?;
            if inner.params != args.len() {
                return None;
            }
            let inner_parts = resolve(definitions, &inner.body, depth + 1)?;
            let mut out = Vec::new();
            for part in inner_parts {
                match part {
                    Part::Lit(text) => out.push(Part::Lit(text)),
                    Part::Param(index) => out.push(args.get(index)?.clone()),
                }
            }
            Some(out)
        }
    }
}

/// `(name, definition)` for every single-arm, literal-only `macro_rules!` in `text`.
fn macro_definitions(text: &str) -> Vec<(String, Definition)> {
    let mut out = Vec::new();
    let mut search = 0;
    while let Some(rel) = text[search..].find("macro_rules!") {
        let at = search + rel + "macro_rules!".len();
        search = at;
        let rest = &text[at..];
        let name_start = at + (rest.len() - rest.trim_start().len());
        let name_end = name_start
            + text[name_start..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(0);
        if name_end == name_start {
            continue;
        }
        let name = text[name_start..name_end].to_string();
        let Some(open) = text[name_end..].find(['{', '(', '[']).map(|n| name_end + n) else {
            continue;
        };
        let Some(close) = call_end(text, open + 1) else {
            continue;
        };
        search = close;
        if let Some(definition) = parse_rules(&text[open + 1..close]) {
            out.push((name, definition));
        }
    }
    out
}

/// One arm `($a:literal, $b:literal) => { … }`, and only one.
fn parse_rules(rules: &str) -> Option<Definition> {
    let rules = strip_comments(rules);
    let rules = rules.trim();
    let open = rules.find(['(', '[', '{'])?;
    if !rules[..open].trim().is_empty() {
        return None;
    }
    let matcher_end = call_end(rules, open + 1)?;
    let matcher = &rules[open + 1..matcher_end];
    let rest = rules[matcher_end + 1..]
        .trim_start()
        .strip_prefix("=>")?
        .trim_start();
    let body_open = rest.find(['(', '[', '{'])?;
    if body_open != 0 {
        return None;
    }
    let body_end = call_end(rest, 1)?;
    let after = rest[body_end + 1..].trim().trim_start_matches(';').trim();
    if !after.is_empty() {
        return None; // a second arm: which one a call takes is up to the matcher
    }
    let params: Vec<String> = matcher
        .split(',')
        .map(str::trim)
        .filter(|piece| !piece.is_empty())
        .map(|piece| {
            let (name, kind) = piece.strip_prefix('$')?.split_once(':')?;
            matches!(kind.trim(), "literal" | "expr" | "tt").then(|| name.trim().to_string())
        })
        .collect::<Option<_>>()?;
    let body = parse_body(rest[1..body_end].trim(), &params)?;
    Some(Definition {
        params: params.len(),
        body,
    })
}

fn parse_body(body: &str, params: &[String]) -> Option<Body> {
    let mut body = body.trim().trim_end_matches(';').trim();
    // `{ … }` around the expression, as many movesets write it.
    while let Some(inner) = body.strip_prefix('{').and_then(|b| b.strip_suffix('}')) {
        body = inner.trim().trim_end_matches(';').trim();
    }
    if let Some(at) = body.find("Hash40::new(") {
        let path = &body[..at];
        if !path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
        {
            return None;
        }
        let open = at + "Hash40::new(".len();
        let close = call_end(body, open)?;
        if !body[close + 1..].trim().is_empty() {
            return None;
        }
        let inner = body[open..close].trim();
        let pieces: Vec<&str> = match inner.strip_prefix("concat!(") {
            Some(rest) => split_args(rest.strip_suffix(')')?),
            None => vec![inner],
        };
        return pieces
            .into_iter()
            .map(|piece| part(piece, params))
            .collect::<Option<Vec<_>>>()
            .map(Body::Hash);
    }
    // `$crate::other!(…)` / `other!(…)`.
    let bang = body.find("!(")?;
    let name_path = body[..bang].trim_start_matches("$crate::");
    let name = name_path.rsplit("::").next()?.to_string();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let close = call_end(body, bang + 2)?;
    if !body[close + 1..].trim().is_empty() {
        return None;
    }
    let args = split_args(&body[bang + 2..close])
        .into_iter()
        .map(|piece| part(piece, params))
        .collect::<Option<Vec<_>>>()?;
    Some(Body::Nested { name, args })
}

fn part(piece: &str, params: &[String]) -> Option<Part> {
    let piece = piece.trim();
    if let Some(name) = piece.strip_prefix('$') {
        return params.iter().position(|p| p == name).map(Part::Param);
    }
    string_literal(piece).map(Part::Lit)
}

fn string_literal(piece: &str) -> Option<String> {
    let inner = piece.trim().strip_prefix('"')?.strip_suffix('"')?;
    (!inner.contains(['"', '\\'])).then(|| inner.to_string())
}

/// `name!("a", "b")` → `("name", ["a", "b"])`, path stripped; literal arguments only.
fn parse_call(call: &str) -> Option<(&str, Vec<&str>)> {
    let bang = call.find('!')?;
    let path = &call[..bang];
    let name = path.rsplit("::").next()?;
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    if !path
        .trim_start_matches("$crate::")
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
    {
        return None;
    }
    let rest = &call[bang + 1..];
    let open = rest.chars().next()?;
    if !matches!(open, '(' | '[' | '{') {
        return None;
    }
    let close = call_end(rest, 1)?;
    if close + 1 != rest.len() {
        return None;
    }
    let args = split_args(&rest[1..close])
        .into_iter()
        .map(|arg| {
            let arg = arg.trim();
            let inner = arg.strip_prefix('"')?.strip_suffix('"')?;
            (!inner.contains(['"', '\\'])).then_some(inner)
        })
        .collect::<Option<Vec<_>>>()?;
    Some((name, args))
}

/// Split at top-level commas; a trailing comma leaves no empty piece.
fn split_args(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut pieces = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let mut i = 0;
    while i < text.len() {
        if let Some(next) = skip_literal_or_comment(text, i) {
            i = next;
            continue;
        }
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                pieces.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if !text[start..].trim().is_empty() {
        pieces.push(&text[start..]);
    }
    pieces
}

/// The offset of the bracket closing the group whose interior starts at `from`.
fn call_end(text: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 1i32;
    let mut i = from;
    while i < text.len() {
        if let Some(next) = skip_literal_or_comment(text, i) {
            i = next;
            continue;
        }
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
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

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with("//") || text[i..].starts_with("/*") {
            i = skip_literal_or_comment(text, i).unwrap_or(text.len());
            out.push(' ');
            continue;
        }
        if let Some(next) = skip_literal_or_comment(text, i) {
            out.push_str(&text[i..next]);
            i = next;
            continue;
        }
        let ch = text[i..].chars().next().expect("in bounds");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shy Guy's actual definitions: two direct `concat!` macros, and `vc!`/`se!` going through
    /// a two-argument `fighter_vc!`/`fighter_se!` declared in another module.
    const SHY_GUY: &str = r#"
#[macro_export]
macro_rules! fighter_vc {
    ($fighter:literal, $tone:literal) => {
        smash::phx::Hash40::new(concat!("vc_", $fighter, "_", $tone))
    };
}
#[macro_export]
macro_rules! fighter_se {
    ($fighter:literal, $tone:literal) => {
        smash::phx::Hash40::new(concat!("se_", $fighter, "_", $tone))
    };
}
/// `cmn!("swing_03")` is `Hash40::new("se_common_swing_03")`.
#[macro_export]
macro_rules! cmn {
    ($tone:literal) => { smash::phx::Hash40::new(concat!("se_common_", $tone)) };
}
macro_rules! vc {
    ($tone:literal) => { $crate::fighter_vc!("pacman", $tone) };
}
macro_rules! se {
    ($tone:literal) => { $crate::fighter_se!("pacman", $tone) };
}
macro_rules! rb {
    ($kind:literal) => { smash::phx::Hash40::new(concat!("rbkind_", $kind)) };
}
/// Not a hash macro: two arms.
macro_rules! two {
    ($a:literal) => { 1 };
    ($a:literal, $b:literal) => { 2 };
}
"#;

    fn shy_guy() -> HashMacros {
        HashMacros::scan([SHY_GUY])
    }

    #[test]
    fn shy_guys_sound_macros_resolve_to_their_names() {
        let macros = shy_guy();
        assert_eq!(
            macros.expand_call(r#"cmn!("swing_06")"#).as_deref(),
            Some("se_common_swing_06")
        );
        assert_eq!(
            macros.expand_call(r#"se!("jump01")"#).as_deref(),
            Some("se_pacman_jump01")
        );
        assert_eq!(
            macros.expand_call(r#"vc!("attack01")"#).as_deref(),
            Some("vc_pacman_attack01")
        );
        assert_eq!(
            macros.expand_call(r#"rb!("attackm")"#).as_deref(),
            Some("rbkind_attackm")
        );
        assert_eq!(
            macros
                .expand_call(r#"crate::fighter_se!("mario", "x")"#)
                .as_deref(),
            Some("se_mario_x")
        );
        assert_eq!(macros.expand_call(r#"two!("a")"#), None);
        assert_eq!(
            macros.expand_call(r#"cmn!(TONE)"#),
            None,
            "only literals resolve"
        );
    }

    #[test]
    fn a_script_reads_with_the_macros_spelled_out_and_nothing_else_touched() {
        let body = r#"    macros::PLAY_SE(agent, cmn!("swing_06")); // cmn!("not_me")
    macros::PLAY_STEP_FLIPPABLE(agent, se!("step_left_m"), se!("step_right_m"));
    println!("{}", "vc!(\"quoted\")");
    macros::PLAY_SE(agent, unknown!("x"));"#;
        let expanded = shy_guy().expand_in(body).into_owned();
        assert_eq!(
            expanded,
            r#"    macros::PLAY_SE(agent, Hash40::new("se_common_swing_06")); // cmn!("not_me")
    macros::PLAY_STEP_FLIPPABLE(agent, Hash40::new("se_pacman_step_left_m"), Hash40::new("se_pacman_step_right_m"));
    println!("{}", "vc!(\"quoted\")");
    macros::PLAY_SE(agent, unknown!("x"));"#
        );
    }

    #[test]
    fn an_edit_is_written_back_in_the_authors_own_macro() {
        let macros = shy_guy();
        let file = r#"macros::PLAY_SE(agent, se!("jump01")); macros::PLAY_SE(agent, cmn!("a"));"#;
        // Same macro, new literal.
        assert_eq!(
            macros.spell(r#"se!("jump01")"#, "se_pacman_jump02", file),
            r#"se!("jump02")"#
        );
        // Same family no longer fits: another of the project's macros does.
        assert_eq!(
            macros.spell(r#"se!("jump01")"#, "se_common_swing_01", file),
            r#"cmn!("swing_01")"#
        );
        // A plain `Hash40::new` the author wrote stays one, even when a macro would fit.
        assert_eq!(
            macros.spell(r#"Hash40::new("se_pacman_x")"#, "se_pacman_y", file),
            r#"Hash40::new("se_pacman_y")"#
        );
        assert_eq!(
            macros.spell(r#"smash::phx::Hash40::new("a")"#, "b", file),
            r#"smash::phx::Hash40::new("b")"#
        );
        // A macro call no macro can say any more falls back to the plain form.
        assert_eq!(
            macros.spell(r#"se!("a")"#, "vc_mario_x", file),
            r#"Hash40::new("vc_mario_x")"#
        );
        assert_eq!(
            macros.spell(r#"crate::se!("a")"#, "se_pacman_b", file),
            r#"crate::se!("b")"#
        );
    }
}
