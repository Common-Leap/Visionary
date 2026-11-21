//! Read ACMD scripts from the user's OWN Rust source instead of the dumped-script mirror,
//! and write edited values back into it.
//!
//! The GitHub mirror holds vanilla scripts. Anyone who has already modded a move is looking
//! at code that no longer matches their game: their move may use `EFFECT_FOLLOW` where
//! vanilla used `EFFECT`, may spawn a graphic vanilla never had, or may not exist upstream
//! at all. Pointing Visionary at the smashline project that actually builds their plugin
//! makes the editor read what they wrote.
//!
//! The write-back direction is deliberately narrow. It rewrites argument VALUES in place and
//! moves only bounded, isolated effect timing blocks: the macro a line calls, the comments, and
//! the whitespace all survive untouched, because the point is to retune code that was already
//! dialled in by hand — not to regenerate someone's source from the editor's model of it.
//! Branches, loops, ambiguous blocks, and structural stop creation/removal are refused and
//! reported, not guessed at. For "throw the original away and emit a fresh project",
//! `mod_export` already exists.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Files bigger than this are not plausible hand-written ACMD and are skipped during the
/// scan, so pointing at a directory that also holds generated data or vendored code does not
/// turn indexing into a multi-second stall.
const MAX_SOURCE_BYTES: u64 = 4 * 1024 * 1024;

/// Directories never worth descending into when looking for ACMD source.
const SKIPPED_DIRS: &[&str] = &["target", ".git", "node_modules", ".cargo"];

// ── Macro call scanning ───────────────────────────────────────────────────────

/// One `macros::NAME(...)` call located in a source file, with a byte span per argument.
///
/// Spans are into the file text and cover the argument's own characters, trimmed of the
/// surrounding whitespace — replacing one leaves the commas and layout exactly as written.
#[derive(Debug, Clone, PartialEq)]
pub struct MacroSite {
    pub name: String,
    /// Span of the whole `macros::NAME(...)` expression.
    pub span: Range<usize>,
    pub args: Vec<Range<usize>>,
}

impl MacroSite {
    pub fn arg<'a>(&self, text: &'a str, index: usize) -> Option<&'a str> {
        self.args.get(index).map(|span| &text[span.clone()])
    }
}

/// Every `macros::…` call in `text[range]`, in document order.
///
/// This is a scanner, not a Rust parser: it tracks string literals, character literals, and
/// both comment forms well enough not to be fooled by a macro name inside one, which is all
/// the accuracy an argument-value rewrite needs.
pub fn scan_macro_sites(text: &str, range: Range<usize>) -> Vec<MacroSite> {
    let mut sites = scan_macro_sites_unscoped(text, range);
    // A project helper standing for one macro call is presented as that call, exactly as the
    // parser reads it (see `crate::project_helpers`). Arguments the helper fixes have no text
    // of their own: they get an empty span at the call's `)`, which no edit is allowed to fill.
    for site in &mut sites {
        let Some(helper) = crate::project_helpers::in_scope(&site.name, site.args.len()) else {
            continue;
        };
        let fixed = site.span.end.saturating_sub(1)..site.span.end.saturating_sub(1);
        site.args = helper
            .slots
            .iter()
            .map(|slot| match slot {
                crate::project_helpers::Slot::Param(index) => site.args[*index].clone(),
                crate::project_helpers::Slot::Fixed(_) => fixed.clone(),
                // The receiver is built from the agent argument; it is never edited.
                crate::project_helpers::Slot::Accessor(index) => site.args[*index].clone(),
            })
            .collect();
        site.name = helper.target;
    }
    sites
}

/// [`scan_macro_sites`] without the project-helper view: every call exactly as written.
pub(crate) fn scan_macro_sites_unscoped(text: &str, range: Range<usize>) -> Vec<MacroSite> {
    const PREFIX: &str = "macros::";
    let bytes = text.as_bytes();
    let mut sites = Vec::new();
    let mut i = range.start;
    while i < range.end {
        // Skip over anything that is not live code before testing for the prefix.
        if let Some(next) = skip_rust_literal_or_comment(text, i) {
            i = next;
            continue;
        }
        // `ATTACK(agent, …)` with no `macros::` in front: a project that glob-imports
        // `smash_script::macros::*` writes every call this way, and across the corpus that is
        // most of them. The name span stands in for the prefix so the site is the same shape.
        if let Some(name) = bare_macro_name_at(text, i, range.end) {
            if let Some((args, close)) = split_call_args(text, name.end + 1, range.end) {
                sites.push(MacroSite {
                    name: text[name.clone()].to_string(),
                    span: i..close + 1,
                    args,
                });
                i = close + 1;
                continue;
            }
        }
        if !text[i..range.end].starts_with(PREFIX) {
            // A trail written in the raw `effect(*MA_MSC_CMD_…, …)` form is a rewritable call
            // too, and it has to be scanned here rather than anywhere else: `call_macro_ordinals`
            // counts the `EffectCall` the parser makes from it, so a site missing here would
            // renumber every later call and land the next edit on the wrong line. See
            // `data::RAW_TRAIL_COMMANDS`.
            if let Some(site) = scan_raw_trail_site(text, i, range.end) {
                i = site.span.end;
                sites.push(site);
                continue;
            }
            i += 1;
            while i < range.end && !text.is_char_boundary(i) {
                i += 1;
            }
            continue;
        }
        let name_start = i + PREFIX.len();
        let mut name_end = name_start;
        while name_end < range.end
            && (bytes[name_end].is_ascii_alphanumeric() || bytes[name_end] == b'_')
        {
            name_end += 1;
        }
        if name_end == name_start || name_end >= range.end || bytes[name_end] != b'(' {
            i = name_end.max(i + 1);
            continue;
        }
        let Some((args, close)) = split_call_args(text, name_end + 1, range.end) else {
            i = name_end + 1;
            continue;
        };
        sites.push(MacroSite {
            name: text[name_start..name_end].to_string(),
            span: i..close + 1,
            args,
        });
        i = close + 1;
    }
    sites
}

/// The name of an ACMD macro called WITHOUT a path at `i` (`ATTACK(agent, …)`), if one starts
/// there.
///
/// smash_script's ACMD macros are all upper case, and nothing else an ACMD body calls is: lua
/// constants are dereferenced (`*FOO`), never called, and types are CamelCase. So an upper-case
/// identifier of at least four characters, directly followed by `(`, that is not itself the end
/// of a path (`sv_animcmd::EFFECT(`), a method (`.ATTACK(`), a macro invocation (`FOO!(`) or a
/// function being declared (`fn PLAY_RANDOM_SE(`), is a bare macro call. The parsers and the
/// write-back scanner both use this one test, so they always agree on which calls exist.
pub(crate) fn bare_macro_name_at(text: &str, i: usize, limit: usize) -> Option<Range<usize>> {
    let bytes = text.as_bytes();
    if i >= limit || !bytes[i].is_ascii_uppercase() || !text.is_char_boundary(i) {
        return None;
    }
    if text[..i]
        .chars()
        .next_back()
        .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '.' | '$' | '#'))
    {
        return None;
    }
    let mut end = i;
    while end < limit && (bytes[end].is_ascii_uppercase() || bytes[end].is_ascii_digit() || bytes[end] == b'_') {
        end += 1;
    }
    if end - i < 4 || bytes.get(end) != Some(&b'(') {
        return None;
    }
    let before = text[..i].trim_end();
    if before.ends_with("fn")
        && !before[..before.len() - 2]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        return None;
    }
    Some(i..end)
}

/// `source` with every bare ACMD macro call given the `macros::` path the parsers look for.
///
/// Only the parsers' view changes: they match `macros::NAME(` throughout, and a project that
/// writes `ATTACK(agent, …)` would otherwise read as nothing but raw lines. The file itself is
/// never rewritten this way — write-back addresses calls through [`scan_macro_sites`], which
/// recognises the bare spelling directly and edits only argument text.
pub(crate) fn qualify_bare_macros(source: &str) -> std::borrow::Cow<'_, str> {
    // ACMD macros take the agent first. Other upper-case calls (the named-argument msc forms
    // such as `ATK_SET_SHIELD_SETOFF_MUL2(ID=3, …)`) are not `macros::` calls and stay as written.
    let agent = crate::acmd::agent_param_name(source).unwrap_or_else(|| "agent".to_string());
    let takes_agent = |open: usize| {
        let args = &source[open + 1..];
        let first = args.split([',', ')']).next().unwrap_or("").trim();
        first == agent || first == "agent"
    };
    let mut inserts: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < source.len() {
        if let Some(next) = skip_trivia(source, i) {
            i = next;
            continue;
        }
        if let Some(name) = bare_macro_name_at(source, i, source.len()) {
            if takes_agent(name.end) {
                inserts.push(i);
            }
            i = name.end;
            continue;
        }
        if !source.is_char_boundary(i) {
            i += 1;
            continue;
        }
        i += 1;
    }
    if inserts.is_empty() {
        return std::borrow::Cow::Borrowed(source);
    }
    let mut out = String::with_capacity(source.len() + inserts.len() * 8);
    let mut last = 0;
    for at in inserts {
        out.push_str(&source[last..at]);
        out.push_str("macros::");
        last = at;
    }
    out.push_str(&source[last..]);
    std::borrow::Cow::Owned(out)
}

/// Whether `line` opens an `if …is_excute(…) {` block, however the project spells the path.
pub(crate) fn opens_is_excute(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("if ") && trimmed.contains("is_excute(") && line.contains('{')
}

/// Every direct call to `name(...)` in `text[range]`, in document order.
///
/// This covers the small number of generated helpers that deliberately do not live under the
/// `macros::` namespace. It shares the same literal/comment and balanced-argument handling as
/// [`scan_macro_sites`], while excluding a function declaration whose name happens to match.
fn scan_named_sites(text: &str, name: &str, range: Range<usize>) -> Vec<MacroSite> {
    let bytes = text.as_bytes();
    let mut sites = Vec::new();
    let mut i = range.start;
    while i < range.end {
        if let Some(next) = skip_rust_literal_or_comment(text, i) {
            i = next;
            continue;
        }
        if !text.is_char_boundary(i) || !text[i..range.end].starts_with(name) {
            i += 1;
            while i < range.end && !text.is_char_boundary(i) {
                i += 1;
            }
            continue;
        }

        // Do not match a qualified or longer identifier that merely ends with `name`.
        if text[..i]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':' || c == '.')
        {
            i += name.len();
            continue;
        }

        let name_end = i + name.len();
        let mut open = name_end;
        while open < range.end && bytes[open].is_ascii_whitespace() {
            open += 1;
        }
        if open >= range.end || bytes[open] != b'(' {
            i = name_end.max(i + 1);
            continue;
        }

        // The generated helper is nested in the exported ACMD function. Its declaration has the
        // same token sequence as a call after the name, but it is not an editable event site.
        if text[..i].trim_end().ends_with("fn") {
            i = open + 1;
            continue;
        }

        let Some((args, close)) = split_call_args(text, open + 1, range.end) else {
            i = open + 1;
            continue;
        };
        sites.push(MacroSite {
            name: name.to_string(),
            span: i..close + 1,
            args,
        });
        i = close + 1;
    }
    sites
}

/// A raw `effect(*MA_MSC_CMD_…, …)` trail call starting at `i`, as a site the rewriters can use.
///
/// Only the commands in [`crate::data::RAW_TRAIL_COMMANDS`] match, and the returned `args`
/// include the command id at index 0 — the slot `agent` fills in a `macros::` call — so a trail
/// written either way is addressed by the same slot numbers.
fn scan_raw_trail_site(text: &str, i: usize, limit: usize) -> Option<MacroSite> {
    const HEAD: &str = "effect";
    let rest = text.get(i..limit)?;
    let after = rest.strip_prefix(HEAD)?;
    // `effect` has to be the whole identifier. Without this, `my_effect(`, `sub_effect(` and a
    // qualified `foo::effect(` all match and produce a site in the middle of another call.
    if text[..i]
        .chars()
        .next_back()
        .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':')
    {
        return None;
    }
    let open = i + HEAD.len() + (after.len() - after.trim_start().len());
    if text.as_bytes().get(open) != Some(&b'(') {
        return None;
    }
    let (args, close) = split_call_args(text, open + 1, limit)?;
    let id = args.first().map(|span| &text[span.clone()])?;
    let name = crate::data::raw_trail_command(id)?;
    Some(MacroSite {
        name: name.to_string(),
        span: i..close + 1,
        args,
    })
}

/// The raw trail call on `line`, if it holds one, as the site the rewriters would find.
///
/// The effect parser uses this to decide whether to type a line, so it types exactly the lines
/// `scan_macro_sites` can locate a site in. A looser test on the parser side (`contains`, say)
/// would accept `sub_effect(*MA_MSC_CMD_…)` that the scanner rejects, produce an `EffectCall`
/// with no site behind it, and shift every later call ordinal onto the wrong line.
pub fn raw_trail_line(line: &str) -> Option<MacroSite> {
    line.match_indices("effect(")
        .find_map(|(i, _)| scan_raw_trail_site(line, i, line.len()))
}

/// If `i` starts a comment, string, or char literal, the offset just past it.
///
/// The scanners walk byte by byte, so `i` can land inside a multi-byte character — a
/// non-ASCII identifier in the user's code is enough. A continuation byte can never start
/// any of these, and returning early keeps the `&text[i..]` below from panicking on it.
pub(crate) fn skip_trivia(text: &str, i: usize) -> Option<usize> {
    if !text.is_char_boundary(i) {
        return None;
    }
    let rest = &text[i..];
    if let Some(body) = rest.strip_prefix("//") {
        return Some(i + 2 + body.find('\n').map_or(body.len(), |n| n + 1));
    }
    if rest.starts_with("/*") {
        let bytes = text.as_bytes();
        let mut cursor = i + 2;
        let mut depth = 1usize;
        while cursor < bytes.len() {
            if bytes[cursor..].starts_with(b"/*") {
                depth += 1;
                cursor += 2;
            } else if bytes[cursor..].starts_with(b"*/") {
                depth -= 1;
                cursor += 2;
                if depth == 0 {
                    return Some(cursor);
                }
            } else {
                cursor += 1;
            }
        }
        return Some(text.len());
    }
    if rest.starts_with('"') {
        return Some(i + string_literal_len(rest));
    }
    // A lifetime (`'a`) also starts with a quote, so only treat this as a char literal when
    // it actually closes like one.
    if rest.starts_with('\'') {
        let len = char_literal_len(rest)?;
        return Some(i + len);
    }
    None
}

/// Whether `text` ends inside a `/* … */` comment it opened and never closed.
///
/// The line-oriented parsers in `acmd` use this to keep a block comment that spans several lines
/// together as one line, so the call it comments out is never read as code.
pub(crate) fn ends_in_block_comment(text: &str) -> bool {
    let mut i = 0;
    while i < text.len() {
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        if let Some(body) = text[i..].strip_prefix("/*") {
            if !body.contains("*/") {
                return true;
            }
        }
        i = skip_trivia(text, i).unwrap_or(i + 1);
    }
    false
}

fn string_literal_len(rest: &str) -> usize {
    let bytes = rest.as_bytes();
    let mut j = 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b'"' => return j + 1,
            _ => j += 1,
        }
    }
    bytes.len()
}

fn char_literal_len(rest: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    if bytes.first() != Some(&b'\'') {
        return None;
    }
    let end = if bytes.get(1) == Some(&b'\\') {
        match bytes.get(2)? {
            b'u' if bytes.get(3) == Some(&b'{') => {
                let close = rest[4..].find('}')? + 4;
                let digits = &rest[4..close];
                if digits.is_empty()
                    || !digits
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() || byte == b'_')
                {
                    return None;
                }
                close + 1
            }
            b'x' => {
                if !bytes.get(3..5)?.iter().all(u8::is_ascii_hexdigit) {
                    return None;
                }
                5
            }
            _ => 3,
        }
    } else {
        1 + rest.get(1..)?.chars().next()?.len_utf8()
    };
    (bytes.get(end) == Some(&b'\'')).then_some(end + 1)
}

/// Split a call's arguments starting just after its `(`. Returns the argument spans and the
/// offset of the matching `)`.
fn split_call_args(text: &str, open: usize, limit: usize) -> Option<(Vec<Range<usize>>, usize)> {
    let bytes = text.as_bytes();
    let mut args = Vec::new();
    let mut depth = 0usize;
    let mut start = open;
    let mut i = open;
    while i < limit {
        if let Some(next) = skip_rust_literal_or_comment(text, i) {
            i = next;
            continue;
        }
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' if depth == 0 => {
                push_arg(text, start..i, &mut args);
                return Some((args, i));
            }
            // Saturating, because a truncated or malformed call would otherwise underflow
            // and panic rather than simply failing to match.
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                push_arg(text, start..i, &mut args);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Record `span` with its surrounding whitespace trimmed off.
///
/// An empty span is either a call with no arguments or the slot after a trailing comma;
/// neither is an argument, and numbering them would shift every following slot.
fn push_arg(text: &str, span: Range<usize>, args: &mut Vec<Range<usize>>) {
    let raw = &text[span.clone()];
    if raw.trim().is_empty() {
        return;
    }
    let lead = raw.len() - raw.trim_start().len();
    let trail = raw.len() - raw.trim_end().len();
    args.push(span.start + lead..span.end - trail);
}

// ── Project indexing ──────────────────────────────────────────────────────────

/// One ACMD function located in the user's source.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptSite {
    pub file: PathBuf,
    /// The Rust function that owns the script registration.
    pub function: String,
    /// The `unsafe extern "C" fn …` item, from `fn` through its closing brace.
    pub span: Range<usize>,
    /// Set when the registered function only forwards to another function in the project,
    /// such as `game_attacks4reverse(agent) { game_attacks4_impl(agent, Facing::Reverse) }`.
    pub delegate: Option<Delegate>,
}

/// One smashline status registration in the user's source: `.status(Main, KIND, f)` or
/// `.on_start(f)`. Articles (fighter objects, projectiles) are agents like any other, so this
/// is how an article's size, speed and life are found: they live in its status code.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusSite {
    /// The status kind exactly as written (`*WEAPON_X_STATUS_KIND_SHOOT`, `0`); empty for `on_start`.
    pub kind_expr: String,
    /// The last path segment of the phase (`Pre`, `Main`, `End`, `Init`, `Exec`, ...), or `Start`
    /// for `on_start`.
    pub phase: String,
    /// The Rust function registered.
    pub function: String,
    pub file: PathBuf,
    /// The function item, from its line start through its closing brace.
    pub span: Range<usize>,
}

/// A status registration the index saw but could not turn into a [`StatusSite`], kept so it is
/// shown to the user rather than silently missing.
#[derive(Debug, Clone, PartialEq)]
pub struct UnresolvedStatus {
    /// The agent, when it is known (a no-op status on a known agent, say).
    pub agent_hint: Option<String>,
    pub kind_expr: String,
    pub phase: String,
    pub file: PathBuf,
    /// 1-based line of the registration.
    pub line: usize,
    /// One sentence on why no site was recorded.
    pub reason: String,
}

/// The shared implementation a forwarding script calls, as this caller sees it.
///
/// The wrapper itself has no body worth showing, so reads use the callee's body with this
/// caller's arguments substituted wherever that is statically certain. Writes are a different
/// matter: a body shared by several scripts, or one shown with substituted values, does not
/// correspond to any single span of source, so [`Delegate::write_refusal`] refuses it rather
/// than letting one variant's edit silently change the others.
#[derive(Debug, Clone, PartialEq)]
pub struct Delegate {
    pub file: PathBuf,
    /// The called function's name.
    pub function: String,
    /// The callee's whole item, from its line start through its closing brace.
    pub span: Range<usize>,
    /// How many indexed scripts run this same body: forwarding wrappers, plus the callee
    /// itself when it is registered as a script too.
    pub callers: usize,
    /// The wrapper's header over the callee's body, with this caller's arguments resolved.
    pub body: String,
    /// Whether `body` differs from the callee's source because values were substituted.
    pub substituted: bool,
    /// Values that depend on this caller's arguments but could not be resolved statically.
    pub unresolved: Vec<String>,
}

impl Delegate {
    /// Why an edit to the script `script` (registered as `wrapper`) cannot be written back
    /// into this callee, if it cannot.
    pub fn write_refusal(&self, script: &str, wrapper: &str) -> Option<String> {
        let location = format!("{} in {}", self.function, self.file.display());
        if self.callers > 1 {
            return Some(format!(
                "{script}: {wrapper} forwards to {location}, which {} scripts share; an edit \
                 here would change all of them, so edit the shared function directly",
                self.callers
            ));
        }
        if self.substituted || !self.unresolved.is_empty() {
            return Some(format!(
                "{script}: {wrapper} forwards to {location} with arguments the editor resolved \
                 for display; writing back would replace the parameterized source, so edit the \
                 shared function directly"
            ));
        }
        None
    }
}

/// One script identity found in more than one source function.
///
/// An ambiguous identity is kept out of [`SourceIndex::script`] so the editor cannot read or
/// write whichever file happened to win a filesystem sort. The locations remain available for a
/// useful warning and for the author to resolve in their project.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptConflict {
    pub fighter: String,
    pub script: String,
    pub sites: Vec<ScriptSite>,
}

impl ScriptConflict {
    pub fn description(&self) -> String {
        let locations = self
            .sites
            .iter()
            .map(|site| format!("{}::{}", site.file.display(), site.function))
            .collect::<Vec<_>>()
            .join(", ");
        // Two copies of one project (a `dist/` source drop beside the live crate) is the usual
        // cause, and the fix is to link the crate actually built rather than edit either file.
        let crates: Vec<PathBuf> = self
            .sites
            .iter()
            .filter_map(|site| {
                site.file
                    .ancestors()
                    .find(|dir| dir.join("Cargo.toml").is_file())
                    .map(Path::to_path_buf)
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        if crates.len() > 1 {
            return format!(
                "{}: {} is defined by {} separate crates ({}) — link the folder of the one you build instead of the folder holding all of them",
                self.fighter,
                self.script,
                crates.len(),
                crates
                    .iter()
                    .map(|dir| dir.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        format!(
            "{}: {} is ambiguous; found it in {} — resolve the duplicate before editing it",
            self.fighter, self.script, locations
        )
    }
}

/// Where a fighter's scripts live in the user's project.
#[derive(Debug, Default, Clone)]
pub struct FighterScripts {
    /// Unambiguous ACMD script name (`game_attackairn`, `effect_attackairn`) → its function.
    pub scripts: HashMap<String, ScriptSite>,
}

/// What a linked project has to say about one move, and what it leaves to the mirror.
#[derive(Debug, Clone)]
pub struct ProjectScript {
    /// The project's own functions, concatenated, verbatim.
    pub body: String,
    /// The [`crate::acmd::SCRIPT_PREFIXES`] entries `body` covers.
    pub covers: Vec<&'static str>,
    /// Categories the linked project defines more than once and therefore cannot be selected
    /// safely. These are deliberately not filled from the mirror.
    pub blocked: Vec<&'static str>,
    /// Values in `body` that forward to a shared implementation and depend on arguments the
    /// index could not resolve statically, one sentence each, for the status line.
    pub notes: Vec<String>,
}

impl ProjectScript {
    /// Whether a category the editor displays is missing, and so worth fetching the mirror for.
    ///
    /// Only the displayed categories count. Waiting on the network to fill in a `sound_`
    /// nothing reads yet would be a straight regression for anyone working offline; see
    /// [`crate::acmd::DISPLAYED_PREFIXES`].
    pub fn needs_mirror(&self) -> bool {
        crate::acmd::DISPLAYED_PREFIXES
            .iter()
            .any(|prefix| !self.covers.contains(prefix) && !self.blocked.contains(prefix))
    }
}

/// An indexed smashline/smash-script project.
#[derive(Debug, Default, Clone)]
pub struct SourceIndex {
    pub root: PathBuf,
    /// Fighter name as Visionary knows it (`mario`) → its scripts.
    pub fighters: HashMap<String, FighterScripts>,
    /// Files that were read during the scan, for change detection.
    pub files: Vec<PathBuf>,
    /// Duplicate script identities that were found but deliberately not selected.
    pub conflicts: Vec<ScriptConflict>,
    /// The project's own `Hash40` shorthand macros (`cmn!`, `se!`, …). See [`crate::hash_macros`].
    pub hash_macros: crate::hash_macros::HashMacros,
    /// Numeric `const`s used as ACMD values. See [`crate::project_consts`].
    pub consts: std::sync::Arc<crate::project_consts::ProjectConsts>,
    /// Helper functions that stand for one macro call. See [`crate::project_helpers`].
    pub helpers: std::sync::Arc<crate::project_helpers::ProjectHelpers>,
    /// Params the moveset overrides in code (`landing_attack_air_frame_f: 8.0`), for frame data.
    pub param_overrides: HashMap<String, f64>,
    /// Scales each fighter's frame callback sets (`ModelModule::set_scale` and its siblings),
    /// by fighter. See [`crate::fighter_scale`].
    pub scales: HashMap<String, crate::fighter_scale::SourceScales>,
    /// A `fighter_param` `scale` the moveset overrides through `param_config`.
    pub param_scale: Option<f32>,
    /// Fighters the crate installs only on chosen costume slots: every `Agent::new("falco")`
    /// it builds is narrowed with `.set_costume(…)`. Empoleon plays on Falco's c80–c87 this
    /// way, so its scripts are that slot character's, and vanilla Falco's own slots keep the
    /// game's. See [`scan_agent_gating`].
    pub costume_gated: HashSet<String>,
    /// Agent (`dedede_fireball`, `pacman`) -> the status scripts the project registers on it.
    /// A registration on a parameter agent (`fn install(agent: &mut Agent)`) is attributed through
    /// the module that installs it, as ACMD scripts are.
    pub statuses: HashMap<String, Vec<StatusSite>>,
    /// Status registrations that could not be attributed to an agent or a function.
    pub unresolved_statuses: Vec<UnresolvedStatus>,
}

/// Project-wide facts the per-file indexing pass needs.
#[derive(Default)]
struct ProjectFacts {
    /// How many ACMD-shaped functions share each name across the project.
    definitions: HashMap<String, usize>,
    /// Every script name some registration call names explicitly.
    registered: HashSet<String>,
}

/// A script registration found anywhere in the selected project. The owning fighter is retained
/// when the registration's source scope identifies exactly one, which lets an installer module
/// safely connect a function in another module.
#[derive(Debug, Clone)]
struct RegisteredScript {
    script: String,
    fighter: Option<String>,
    /// The fighter is the agent the registration call is made on, not a guess from the
    /// registering file's declarations.
    exact: bool,
}

/// The same function text with its header renamed to `script`, when the project calls it
/// something else.
///
/// A project's function can be called anything: `agent.acmd("game_attackairn", my_nair, …)` and
/// `#[acmd_script(script = "game_attackairn")] fn my_nair` are both ordinary. The index already
/// knows which script such a function *is* — but everything downstream of [`SourceIndex::
/// script_source`] (`parse_acmd_script`, `parse_effect_script`, `parse_sound_script`,
/// `parse_expression_script`, and the category split in `create_missing_scripts`) finds a
/// category by looking for `fn game_`/`fn effect_`/… in the header. Handing those readers the
/// custom name makes the move read as empty: the function is present, carries no category, and
/// once any *other* category is in the same body the headerless whole-text fallback in
/// `parse_acmd_script` is off as well.
///
/// So the name the index resolved is written into the copy that goes on screen. Only this
/// in-memory copy: writes go back through [`sync_script`], which patches the file by the site's
/// own span and never sees this text, so the user's chosen name is left alone on disk.
fn header_named(source: &str, script: &str) -> String {
    let Some(fn_at) = source.find("fn ") else {
        return source.to_string();
    };
    let name_at = fn_at + 3;
    let rest = &source[name_at..];
    let name_end = name_at
        + rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
    if &source[name_at..name_end] == script {
        return source.to_string();
    }
    format!("{}{script}{}", &source[..name_at], &source[name_end..])
}

impl SourceIndex {
    /// Walk `root` and index every ACMD function it can attribute to a fighter.
    pub fn build(root: &Path) -> Result<Self> {
        if !root.is_dir() {
            bail!("not a directory: {}", root.display());
        }
        let mut index = SourceIndex {
            root: root.to_path_buf(),
            ..Default::default()
        };
        let mut files = Vec::new();
        collect_rust_files(root, &mut files, 0)?;
        files.sort();

        // Two passes, because the conventional smashline layout splits the two halves of the
        // answer across files: `src/mario/mod.rs` says which fighter this is, and
        // `src/mario/acmd.rs` holds its scripts. Neither file can be indexed alone.
        let mut sources: Vec<(PathBuf, String)> = files
            .iter()
            .filter_map(|file| Some((file.clone(), std::fs::read_to_string(file).ok()?)))
            .collect();
        let texts: HashMap<PathBuf, &str> = sources
            .iter()
            .map(|(file, text)| (file.clone(), text.as_str()))
            .collect();
        // Compiled files above the linked folder. A folder linked inside a crate
        // (`src/emperte/`) is often told which fighter it is only by the crate root
        // (`src/lib.rs` handing it `Agent::new("falco")`), so those files are read for who owns
        // what, though their own scripts are not indexed.
        let mut context: Vec<(PathBuf, String)> = Vec::new();
        // Where a walk up from a script file stops looking for the module that names it.
        let mut scope_root = root.to_path_buf();
        if let Some(compiled) = compiled_files(root, &texts) {
            // Cargo spells paths its own way (drive-letter case, separators); compare by key.
            let keys: HashSet<String> =
                compiled.iter().map(|file| crate::cargo_project::path_key(file)).collect();
            let built = |file: &PathBuf| keys.contains(&crate::cargo_project::path_key(file));
            sources.retain(|(file, _)| built(file));
            files.retain(built);
            let linked: HashSet<String> =
                files.iter().map(|file| crate::cargo_project::path_key(file)).collect();
            let mut outside: Vec<&PathBuf> = compiled
                .iter()
                .filter(|file| !linked.contains(&crate::cargo_project::path_key(file)))
                .collect();
            outside.sort();
            for file in outside {
                // Respell under the linked folder's own ancestors, so the scopes these files
                // declare are the same paths the walk from a linked file meets.
                let key = crate::cargo_project::path_key(file);
                let Some((ancestor, depth)) = root.ancestors().skip(1).find_map(|ancestor| {
                    let prefix = crate::cargo_project::path_key(ancestor);
                    let prefix = prefix.trim_end_matches(['\\', '/']);
                    let rest = key.strip_prefix(prefix)?;
                    rest.starts_with(['\\', '/']).then(|| {
                        (ancestor, rest.split(['\\', '/']).filter(|s| !s.is_empty()).count())
                    })
                }) else {
                    continue;
                };
                let parts: Vec<_> = file.components().collect();
                let Some(tail) = parts.len().checked_sub(depth).map(|start| &parts[start..]) else {
                    continue;
                };
                let respelled = tail.iter().fold(ancestor.to_path_buf(), |dir, part| dir.join(part));
                let Ok(text) = std::fs::read_to_string(&respelled) else {
                    continue;
                };
                if scope_root.starts_with(ancestor) {
                    scope_root = ancestor.to_path_buf();
                }
                context.push((respelled, text));
            }
        }
        // A fighter declaration in `src/fighters/mario/mod.rs` owns every nested module below
        // it, not only Rust files immediately beside it. Keep declarations as directory scopes
        // and resolve a function against its nearest ancestor scope below.
        let mut fighter_scopes: HashMap<PathBuf, Vec<String>> = HashMap::new();
        let mut project_wide: Vec<String> = Vec::new();
        let mut project_registrations: HashMap<String, Vec<RegisteredScript>> = HashMap::new();
        let mut facts = ProjectFacts::default();
        // Fighter → (constructions narrowed to costumes, constructions that are not).
        let mut gating: HashMap<String, (usize, usize)> = HashMap::new();
        for (file, text) in sources.iter().chain(&context) {
            for function in scan_acmd_functions(text) {
                *facts.definitions.entry(function.name).or_default() += 1;
            }
            for (fighter, gated) in scan_agent_gating(text) {
                let counts = gating.entry(fighter).or_default();
                if gated {
                    counts.0 += 1;
                } else {
                    counts.1 += 1;
                }
            }
            let names = scan_fighter_names(text);
            for name in &names {
                if !project_wide.contains(name) {
                    project_wide.push(name.clone());
                }
            }
            // Only a folder that declares something is a scope. An empty entry stopped
            // `fighter_for_file`'s walk at the first folder holding any `.rs` file, so
            // `pacman/acmd/tilts.rs` never reached the `pacman/` that owns it.
            if let Some(dir) = file.parent().filter(|_| !names.is_empty()) {
                let entry = fighter_scopes.entry(dir.to_path_buf()).or_default();
                for name in &names {
                    if !entry.contains(name) {
                        entry.push(name.clone());
                    }
                }
            }
            // `pacman::install(pacman)` where `pacman` is that file's `Agent::new("pacman")`
            // hands the whole `pacman` module to that fighter, even though the installing file
            // declares several agents (Shy Guy's installs `pacman` and three articles).
            for (module, fighter) in scan_install_routes(file, text) {
                let entry = fighter_scopes.entry(module).or_default();
                if !entry.contains(&fighter) {
                    entry.push(fighter);
                }
            }
            let sole_name = (names.len() == 1).then(|| names[0].clone());
            let registrations = scan_registrations(text);
            let last = last_registrations(&registrations);
            for registration in &registrations {
                facts.registered.insert(registration.script.to_string());
                // Replaced by a later registration of the same agent and script in this file.
                if last.get(&(registration.agent.clone(), registration.script))
                    != Some(&registration.function)
                {
                    continue;
                }
                // The agent the call is made on beats the file's own declarations: a file that
                // registers `falco` and `falco_cannon` scripts still knows which is which.
                let fighter = registration.agent.clone().or_else(|| sole_name.clone());
                let scripts = project_registrations
                    .entry(registration.function.to_string())
                    .or_default();
                if !scripts
                    .iter()
                    .any(|known| known.script == registration.script && known.fighter == fighter)
                {
                    scripts.push(RegisteredScript {
                        script: registration.script.to_string(),
                        fighter,
                        exact: registration.agent.is_some(),
                    });
                }
            }
        }
        for (file, text) in &sources {
            index.index_file(
                file,
                text,
                &fighter_scopes,
                &project_wide,
                &project_registrations,
                &facts,
                &scope_root,
            );
        }
        index.resolve_delegates(&sources);
        index.index_statuses(&sources, &fighter_scopes, &scope_root);
        index.hash_macros =
            crate::hash_macros::HashMacros::scan(sources.iter().map(|(_, text)| text.as_str()));
        crate::hash_macros::set_current(index.hash_macros.clone());
        index.consts = std::sync::Arc::new(crate::project_consts::ProjectConsts::scan(&sources));
        index.helpers = std::sync::Arc::new(crate::project_helpers::ProjectHelpers::scan(
            sources.iter().map(|(_, text)| text.as_str()),
        ));
        index.param_overrides = crate::frame_data::source_param_overrides(
            sources.iter().map(|(_, text)| text.as_str()),
        );
        index.scales = crate::fighter_scale::scan(
            &sources,
            |file| fighter_for_file(file, &scope_root, &fighter_scopes),
            &index.consts,
        );
        index.param_scale = crate::fighter_scale::param_config_scale(
            sources.iter().map(|(_, text)| text.as_str()),
        );
        // One agent for every costume is enough to make the fighter's own slots modded too.
        index.costume_gated = gating
            .into_iter()
            .filter(|(_, (gated, ungated))| *gated > 0 && *ungated == 0)
            .map(|(fighter, _)| fighter)
            .collect();
        index.files = files;
        Ok(index)
    }

    /// Whether the crate installs `fighter`'s scripts only on chosen costume slots.
    #[cfg(test)]
    pub fn is_costume_gated(&self, fighter: &str) -> bool {
        self.costume_gated.contains(&normalize_fighter(fighter))
    }

    /// Every script the project has for `fighter` and for its objects (`pacman_bigpacman`), as
    /// (agent, script name, body), the project's hash macros expanded so `Hash40::new` names
    /// read as written in the game's own scripts. A forwarding script's body is what it runs.
    pub fn fighter_script_bodies(&self, fighter: &str) -> Vec<(String, String, String)> {
        let fighter = normalize_fighter(fighter);
        let objects = format!("{fighter}_");
        let unattributed = self.unattributed_scripts().is_some();
        let mut texts: HashMap<&Path, Option<String>> = HashMap::new();
        let mut out = Vec::new();
        for (agent, table) in &self.fighters {
            let own = *agent == fighter || (agent.is_empty() && unattributed);
            if !own && !agent.starts_with(&objects) {
                continue;
            }
            let agent = if own { &fighter } else { agent };
            for (name, site) in &table.scripts {
                let body = match &site.delegate {
                    Some(delegate) => delegate.body.clone(),
                    None => {
                        let text = texts
                            .entry(site.file.as_path())
                            .or_insert_with(|| std::fs::read_to_string(&site.file).ok());
                        // The span came from this file's text; a later edit can shrink it.
                        let Some(source) = text.as_deref().and_then(|t| t.get(site.span.clone()))
                        else {
                            continue;
                        };
                        source.to_owned()
                    }
                };
                let body = self.hash_macros.expand_in(&body).into_owned();
                out.push((agent.clone(), name.clone(), body));
            }
        }
        out.sort();
        out
    }

    /// Total number of indexed scripts, across every fighter.
    pub fn script_count(&self) -> usize {
        self.fighters.values().map(|f| f.scripts.len()).sum()
    }

    /// Number of script identities that need the user to resolve before they can be edited.
    pub fn conflict_count(&self) -> usize {
        self.conflicts.len()
    }

    /// Whether the scan found either an editable script or a conflict worth resolving.
    pub fn has_scripts(&self) -> bool {
        self.script_count() > 0 || self.conflict_count() > 0
    }

    /// The site for one fighter + ACMD script name.
    ///
    /// Tries [`crate::acmd::script_name_aliases`] in order, so a lookup for
    /// `game_attacks3s` (the motion's spelling) finds a project that defines
    /// `game_attacks3` (the game's spelling for most fighters), and vice versa
    /// for Ryu/Ken.
    pub fn script(&self, fighter: &str, script_name: &str) -> Option<&ScriptSite> {
        let fighter = normalize_fighter(fighter);
        let table = self
            .fighters
            .get(&fighter)
            .or_else(|| self.unattributed_scripts())?;
        for candidate in crate::acmd::script_name_aliases(script_name) {
            if let Some(site) = table.scripts.get(&candidate) {
                return Some(site);
            }
        }
        None
    }

    /// Exact-only script lookup, without alias fallback.
    ///
    /// [`Self::script_source`] tries each spelling itself so it knows which one
    /// matched (the header rewrite needs the real name); going through the
    /// alias-aware [`Self::script`] there would find the alias while reporting
    /// the requested spelling.
    fn script_exact(&self, fighter: &str, script_name: &str) -> Option<&ScriptSite> {
        let fighter = normalize_fighter(fighter);
        self.fighters
            .get(&fighter)
            .or_else(|| self.unattributed_scripts())?
            .scripts
            .get(script_name)
    }

    /// A source selection without any fighter metadata can still be useful when it is the
    /// project's *only* script set. Treat that as an explicitly unlabelled single-fighter
    /// project, rather than hashing an arbitrary filename into a fighter identity. The empty
    /// sentinel is exposed only while it remains the project's sole script set.
    fn unattributed_scripts(&self) -> Option<&FighterScripts> {
        (self.fighters.len() == 1)
            .then(|| self.fighters.get(""))
            .flatten()
    }

    /// Whether a selected fighter can use this index, including a single, explicitly
    /// unlabelled source set.
    pub fn has_fighter(&self, fighter: &str) -> bool {
        self.fighters.contains_key(&normalize_fighter(fighter))
            || self.unattributed_scripts().is_some()
    }

    /// Whether all indexed scripts are one deliberately unlabelled source set.
    pub fn has_unattributed_scripts(&self) -> bool {
        self.unattributed_scripts().is_some()
    }

    /// The duplicate locations for one fighter + ACMD script name, if any.
    ///
    /// Alias-aware like [`Self::script`]: a duplicate `game_attacks3s` blocks the
    /// move even when the project also defines the `game_attacks3` spelling.
    pub fn conflict(&self, fighter: &str, script_name: &str) -> Option<&ScriptConflict> {
        let fighter = normalize_fighter(fighter);
        for candidate in crate::acmd::script_name_aliases(script_name) {
            if let Some(found) = self
                .conflicts
                .iter()
                .find(|conflict| conflict.fighter == fighter && conflict.script == candidate)
            {
                return Some(found);
            }
            if self.fighters.len() == 1 && self.fighters.contains_key("") {
                if let Some(found) = self
                    .conflicts
                    .iter()
                    .find(|conflict| conflict.fighter.is_empty() && conflict.script == candidate)
                {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Exact-only conflict lookup, without alias fallback. See [`Self::script_exact`].
    fn conflict_exact(&self, fighter: &str, script_name: &str) -> Option<&ScriptConflict> {
        let fighter = normalize_fighter(fighter);
        self.conflicts
            .iter()
            .find(|conflict| conflict.fighter == fighter && conflict.script == script_name)
            .or_else(|| {
                (self.fighters.len() == 1 && self.fighters.contains_key(""))
                    .then(|| {
                        self.conflicts.iter().find(|conflict| {
                            conflict.fighter.is_empty() && conflict.script == script_name
                        })
                    })
                    .flatten()
            })
    }

    /// Why [`Self::script_source`] has nothing for this move, in a phrase for the status line —
    /// the difference between "this project is not for that fighter" and "this move is not
    /// modded" is the whole diagnosis when a modded move shows vanilla.
    pub fn why_no_script(&self, fighter: &str, move_name: &str) -> String {
        let wanted = normalize_fighter(fighter);
        if self.script_count() == 0 && self.conflict_count() == 0 {
            return "the linked source has no ACMD scripts".into();
        }
        if !self.has_fighter(fighter) {
            let mut agents: Vec<&str> = self
                .fighters
                .keys()
                .map(String::as_str)
                .filter(|name| !name.is_empty())
                .collect();
            agents.sort_unstable();
            let shown = agents.iter().take(6).copied().collect::<Vec<_>>().join(", ");
            let more = agents.len().saturating_sub(6);
            let more = if more > 0 { format!(" +{more}") } else { String::new() };
            return format!("the linked source has no `{wanted}` scripts (it defines {shown}{more})");
        }
        format!("the linked source does not define {wanted}/{move_name}")
    }

    /// The functions this project defines for a move, concatenated into one body the existing
    /// `acmd::parse_*` functions can read — the same shape the dumped scripts have — together
    /// with the list of categories that body actually covers.
    ///
    /// `None` when the project defines nothing at all for the move, so the caller uses the
    /// mirror alone. Any single category is enough for `Some`: what the project does not
    /// define is filled from the mirror by [`crate::acmd::merge_project_over_mirror`], which
    /// is why the coverage list has to come back with the text.
    pub fn script_source(&self, fighter: &str, move_name: &str) -> Option<ProjectScript> {
        let mut body = String::new();
        let mut covers: Vec<&'static str> = Vec::new();
        let mut blocked: Vec<&'static str> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        for prefix in crate::acmd::SCRIPT_PREFIXES {
            let trimmed = prefix.trim_end_matches('_');
            // `attack_s3_s` is `game_attacks3` upstream (except Ryu/Ken's real
            // `game_attacks3s`): try the motion's own spelling first so the
            // exception still wins, then the de-`_s` alias. Exact-only lookups
            // here so the header rewrite uses the spelling that actually
            // matched rather than the one that was asked for.
            let mut resolved: Option<(String, ScriptSite)> = None;
            let mut saw_conflict = false;
            for name in crate::acmd::acmd_script_candidates(trimmed, move_name) {
                if self.conflict_exact(fighter, &name).is_some() {
                    saw_conflict = true;
                    break;
                }
                if let Some(site) = self.script_exact(fighter, &name) {
                    resolved = Some((name, site.clone()));
                    break;
                }
            }
            if saw_conflict {
                blocked.push(prefix);
                continue;
            }
            if let Some((name, site)) = resolved {
                // A forwarding wrapper has no body of its own; show what it runs. The resolved
                // text was built from the same snapshot as the span, at index time.
                if let Some(delegate) = &site.delegate {
                    let text = self.hash_macros.expand_in(&delegate.body);
                    let text = self.helpers.inline(&text);
                    let text = self.consts.substitute(&text);
                    body.push_str(&header_named(&text, &name));
                    body.push_str("\n\n");
                    covers.push(prefix);
                    notes.extend(
                        delegate
                            .unresolved
                            .iter()
                            .map(|reason| format!("{name}: {reason}")),
                    );
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(&site.file) {
                    // The span came from this file's text; a concurrent edit can shrink it.
                    if let Some(source) = text.get(site.span.clone()) {
                        // `cmn!("swing_06")` reads as the name it hashes; the file keeps the
                        // macro, and write-back puts edits back into it.
                        let source = self.hash_macros.expand_in(source);
                        // `RUMBLE_HIT(agent, kind)` reads as the vanilla call it makes.
                        let source = self.helpers.inline(&source);
                        // `WALK_LW_DAMAGE` reads as its value; an edit to it goes to the const.
                        let source = self.consts.substitute(&source);
                        body.push_str(&header_named(&source, &name));
                        body.push_str("\n\n");
                        covers.push(prefix);
                    }
                }
            }
        }
        (!covers.is_empty() || !blocked.is_empty()).then_some(ProjectScript {
            body,
            covers,
            blocked,
            notes,
        })
    }

    /// Point every script whose function only forwards to another project function at that
    /// function's body. See [`Delegate`].
    fn resolve_delegates(&mut self, sources: &[(PathBuf, String)]) {
        let mut functions: HashMap<String, Vec<(usize, PlainFunction)>> = HashMap::new();
        let mut methods: MethodTable = HashMap::new();
        for (file_at, (_, text)) in sources.iter().enumerate() {
            let impls = scan_impl_blocks(text);
            for function in scan_plain_functions(text) {
                let owner = impls.iter().find(|(_, block)| {
                    block.start < function.span.start && function.span.end <= block.end
                });
                if let Some((ty, _)) = owner {
                    let Some(ty) = ty else {
                        continue;
                    };
                    let takes_self = function
                        .params
                        .as_ref()
                        .and_then(|params| params.first())
                        .is_some_and(|(name, _)| name == "self");
                    if takes_self {
                        let arms = parse_self_match(&text[function.body.clone()]);
                        // Two impls defining the same method leave nothing certain to read.
                        methods
                            .entry((ty.clone(), function.name.clone()))
                            .and_modify(|known| *known = None)
                            .or_insert(arms);
                    }
                    continue;
                }
                functions
                    .entry(function.name.clone())
                    .or_default()
                    .push((file_at, function));
            }
        }
        let file_at: HashMap<&Path, usize> = sources
            .iter()
            .enumerate()
            .map(|(at, (file, _))| (file.as_path(), at))
            .collect();
        let registered: HashSet<(PathBuf, String)> = self
            .fighters
            .values()
            .flat_map(|table| table.scripts.values())
            .map(|site| (site.file.clone(), site.function.clone()))
            .collect();

        struct Pending<'a> {
            fighter: String,
            script: String,
            wrapper: ScriptSite,
            forward: Forward,
            callee: Result<(usize, &'a PlainFunction), String>,
        }
        let mut pending: Vec<Pending> = Vec::new();
        for (fighter, table) in &self.fighters {
            for (script, site) in &table.scripts {
                let Some(&at) = file_at.get(site.file.as_path()) else {
                    continue;
                };
                let Some(item) = sources[at].1.get(site.span.clone()) else {
                    continue;
                };
                let Some(forward) = forwarding_call(item) else {
                    continue;
                };
                // A call to something the project does not define is an ordinary one-line
                // script (or a crate helper), not a wrapper the index can see through.
                let callee = match functions.get(&forward.callee).map(Vec::as_slice) {
                    None | Some([]) => continue,
                    Some([(callee_at, callee)]) => {
                        if *callee_at == at && callee.span.start == site.span.start {
                            continue;
                        }
                        Ok((*callee_at, callee))
                    }
                    Some(many) => Err(format!(
                        "forwards to {}, which the project defines {} times; the shared body \
                         cannot be chosen safely",
                        forward.callee,
                        many.len()
                    )),
                };
                pending.push(Pending {
                    fighter: fighter.clone(),
                    script: script.clone(),
                    wrapper: site.clone(),
                    forward,
                    callee,
                });
            }
        }

        let mut callers: HashMap<(usize, usize), HashSet<(PathBuf, String)>> = HashMap::new();
        for entry in &pending {
            if let Ok((at, callee)) = entry.callee {
                callers
                    .entry((at, callee.span.start))
                    .or_default()
                    .insert((entry.wrapper.file.clone(), entry.wrapper.function.clone()));
            }
        }

        let mut resolved = Vec::new();
        for entry in pending {
            let item = &sources[file_at[entry.wrapper.file.as_path()]].1[entry.wrapper.span.clone()];
            let delegate = match entry.callee {
                Err(reason) => Delegate {
                    file: entry.wrapper.file.clone(),
                    function: entry.forward.callee.clone(),
                    span: entry.wrapper.span.clone(),
                    callers: 0,
                    body: item.to_string(),
                    substituted: false,
                    unresolved: vec![reason],
                },
                Ok((at, callee)) => {
                    let (file, text) = &sources[at];
                    let callee_body = &text[callee.body.clone()];
                    let resolution = match &callee.params {
                        Some(params) => resolve_shared_body(
                            callee_body,
                            params,
                            &entry.forward.args,
                            &entry.forward.params,
                            &methods,
                        ),
                        None => Resolution {
                            body: callee_body.to_string(),
                            substituted: false,
                            unresolved: vec![format!(
                                "the parameter list of {} could not be read",
                                callee.name
                            )],
                        },
                    };
                    let mut count = callers
                        .get(&(at, callee.span.start))
                        .map_or(0, HashSet::len);
                    if registered.contains(&(file.clone(), callee.name.clone())) {
                        count += 1;
                    }
                    Delegate {
                        file: file.clone(),
                        function: callee.name.clone(),
                        span: callee.span.clone(),
                        callers: count,
                        body: format!("{}{}", &item[..entry.forward.open], resolution.body),
                        substituted: resolution.substituted,
                        unresolved: resolution.unresolved,
                    }
                }
            };
            resolved.push((entry.fighter, entry.script, delegate));
        }
        for (fighter, script, delegate) in resolved {
            if let Some(site) = self
                .fighters
                .get_mut(&fighter)
                .and_then(|table| table.scripts.get_mut(&script))
            {
                site.delegate = Some(delegate);
            }
        }
    }

    fn index_file(
        &mut self,
        file: &Path,
        text: &str,
        fighter_scopes: &HashMap<PathBuf, Vec<String>>,
        project_wide: &[String],
        project_registrations: &HashMap<String, Vec<RegisteredScript>>,
        facts: &ProjectFacts,
        scope_root: &Path,
    ) {
        let functions = scan_acmd_functions(text);
        if functions.is_empty() {
            return;
        }
        // `agent.acmd("game_attackairn", game_attackairn, …)` names a script explicitly, as
        // does a `#[acmd_script(script = "…")]` attribute; a plain `fn game_attackairn` is
        // its own name by convention.
        //
        // A script registered twice in one file runs whichever function was registered LAST
        // (smashline keeps one function per script name), so only that one is the script.
        let registrations = scan_registrations(text);
        let winner = last_registrations(&registrations);
        // Function → the (agent, script) pairs this file registers it as. The agent is `None`
        // when the call is made on a parameter, and is then resolved from the file's scope.
        let mut named: HashMap<&str, Vec<(Option<String>, String)>> = HashMap::new();
        // Functions this file registers only to have the registration replaced: they run as
        // nothing, and must not fall back to another file's registration or their own name.
        let mut replaced: HashSet<&str> = HashSet::new();
        for registration in &registrations {
            let key = (registration.agent.clone(), registration.script);
            if winner.get(&key) != Some(&registration.function) {
                replaced.insert(registration.function);
                continue;
            }
            let target = (registration.agent.clone(), registration.script.to_string());
            let targets = named.entry(registration.function).or_default();
            if !targets.contains(&target) {
                targets.push(target);
            }
        }

        for function in functions {
            // A status function (`fn stub(fighter: &mut L2CFighterCommon)`) is never an ACMD
            // script, however a same-named ACMD helper elsewhere is registered.
            let header_end = text[function.span.clone()]
                .find('{')
                .map_or(function.span.end, |n| function.span.start + n);
            let header = &text[function.span.start..header_end];
            if !header.contains("L2CAgentBase")
                && ["L2CFighterCommon", "L2CWeaponCommon", "L2CFighterBase"]
                    .iter()
                    .any(|status_type| header.contains(status_type))
            {
                continue;
            }
            // Another file's registration names this function only when nothing else in the
            // project shares its name: `stub` defined in two modules is two functions.
            let registrations = project_registrations
                .get(function.name.as_str())
                .filter(|_| facts.definitions.get(&function.name).copied().unwrap_or(0) <= 1);
            // `(agent, script)` per script this function is. A known agent is the call's own
            // receiver; `None` falls back to the scopes below.
            let targets: Vec<(Option<String>, String)> = if let Some(script) = function.script.clone() {
                vec![(None, script)]
            } else if let Some(targets) = named.get(function.name.as_str()) {
                targets.clone()
            } else if replaced.contains(function.name.as_str()) {
                continue;
            } else if let Some(scripts) = registrations {
                scripts
                    .iter()
                    .map(|registered| {
                        let agent = registered.exact.then(|| registered.fighter.clone()).flatten();
                        (agent, registered.script.clone())
                    })
                    .collect()
            } else if ["game_", "effect_", "sound_", "expression_"]
                .iter()
                .any(|prefix| function.name.starts_with(prefix))
                // Named like a script but never registered, while the project explicitly
                // registers that script name to some other function: dead code, and the
                // registered function is what the game runs.
                && !facts.registered.contains(&function.name)
            {
                vec![(None, function.name.clone())]
            } else {
                continue;
            };
            // Narrowest scope that names exactly ONE fighter wins: the function's own
            // attribute, then its closest enclosing source module, then the whole project for
            // a single-character mod. A scope naming several fighters is ambiguous, and
            // guessing would silently attach a move to the wrong one.
            let sole = |names: &[String]| (names.len() == 1).then(|| names[0].clone());
            let registration_fighter = registrations.and_then(|registered| {
                let mut fighters = registered
                    .iter()
                    .filter_map(|registration| registration.fighter.as_ref())
                    .collect::<Vec<_>>();
                fighters.sort_unstable();
                fighters.dedup();
                (fighters.len() == 1).then(|| fighters[0].clone())
            });
            let fallback = function
                .fighter
                .clone()
                .or_else(|| fighter_for_file(file, scope_root, fighter_scopes))
                .or(registration_fighter)
                .or_else(|| sole(project_wide))
                // A folder of ACMD modules is often shared without the plugin's install
                // module. It has no fighter label to recover, but when the entire selected
                // source set has no labels at all it is still one safely editable set. Store
                // it under the empty sentinel; `script` exposes it only while it remains the
                // sole set, so it can never silently bleed into a named multi-fighter project.
                .or_else(|| project_wide.is_empty().then(String::new));
            for (agent, script_name) in targets {
                let Some(fighter) = agent.or_else(|| fallback.clone()) else {
                    continue;
                };
                self.record_script(
                    &fighter,
                    script_name,
                    ScriptSite {
                        file: file.to_path_buf(),
                        function: function.name.clone(),
                        span: function.span.clone(),
                        delegate: None,
                    },
                );
            }
        }
    }

    /// Record every `.status(..)` / `.on_start(..)` registration. The agent is the call's own
    /// receiver, else the module's fighter the way ACMD scripts find theirs (`status::install(agent)`
    /// in a module that holds an `Agent::new`). The registered function is looked up in its own
    /// file first, then project-wide when its name is defined exactly once. Whatever cannot be
    /// resolved goes to `unresolved_statuses` with the reason.
    fn index_statuses(
        &mut self,
        sources: &[(PathBuf, String)],
        fighter_scopes: &HashMap<PathBuf, Vec<String>>,
        scope_root: &Path,
    ) {
        let mut defined: HashMap<String, Vec<(usize, Range<usize>)>> = HashMap::new();
        for (n, (_, text)) in sources.iter().enumerate() {
            for function in scan_plain_functions(text) {
                defined.entry(function.name).or_default().push((n, function.span));
            }
        }
        for (n, (file, text)) in sources.iter().enumerate() {
            for registration in scan_status_registrations(text) {
                let line = text[..registration.at].matches('\n').count() + 1;
                let agent = registration
                    .agent
                    .clone()
                    .or_else(|| fighter_for_file(file, scope_root, fighter_scopes));
                let unresolved = UnresolvedStatus {
                    agent_hint: agent.clone(),
                    kind_expr: registration.kind_expr.clone(),
                    phase: registration.phase.clone(),
                    file: file.clone(),
                    line,
                    reason: String::new(),
                };
                if registration.noop {
                    self.unresolved_statuses.push(UnresolvedStatus {
                        reason: "registered as a no-op with noop_status!".to_string(),
                        ..unresolved
                    });
                    continue;
                }
                let Some(agent) = agent else {
                    self.unresolved_statuses.push(UnresolvedStatus {
                        reason: "the agent is a parameter and no single module installs it from an Agent::new, or several agents claim the module".to_string(),
                        ..unresolved
                    });
                    continue;
                };
                if !is_identifier(&registration.function) {
                    self.unresolved_statuses.push(UnresolvedStatus {
                        reason: "registered with a closure or expression, not a named function".to_string(),
                        ..unresolved
                    });
                    continue;
                }
                let candidates = defined.get(&registration.function).map(Vec::as_slice).unwrap_or(&[]);
                let own = candidates.iter().filter(|(at, _)| *at == n).collect::<Vec<_>>();
                // `fly::bullet_seed_end` names the function in the `fly` module beside the
                // registering file, however many other modules define the same name.
                let in_module = registration.module.as_deref().map(|module| {
                    let dir = file.parent().unwrap_or(Path::new(""));
                    let stem_dir = file.with_extension("");
                    candidates
                        .iter()
                        .filter(|(at, _)| {
                            let path = &sources[*at].0;
                            [dir, stem_dir.as_path()].iter().any(|base| {
                                *path == base.join(format!("{module}.rs")) || *path == base.join(module).join("mod.rs")
                            })
                        })
                        .collect::<Vec<_>>()
                });
                let found = match (in_module.as_deref(), own.as_slice(), candidates) {
                    (Some([one]), _, _) => Some((*one).clone()),
                    (Some(_), _, _) => None,
                    (None, [one], _) => Some((*one).clone()),
                    (None, [], [one]) => Some(one.clone()),
                    _ => None,
                };
                let Some((at, span)) = found else {
                    let reason = if candidates.is_empty() {
                        "the registered function is not defined in the project source"
                    } else {
                        "the registered function name is defined more than once"
                    };
                    self.unresolved_statuses.push(UnresolvedStatus {
                        reason: reason.to_string(),
                        ..unresolved
                    });
                    continue;
                };
                let site = StatusSite {
                    kind_expr: registration.kind_expr,
                    phase: registration.phase,
                    function: registration.function,
                    file: sources[at].0.clone(),
                    span,
                };
                let sites = self.statuses.entry(agent).or_default();
                if !sites.contains(&site) {
                    sites.push(site);
                }
            }
        }
    }

    /// The status scripts registered on `agent`, each with its function's source text.
    pub fn status_bodies(&self, agent: &str) -> Vec<(&StatusSite, String)> {
        let Some(sites) = self.statuses.get(&normalize_fighter(agent)) else {
            return Vec::new();
        };
        sites
            .iter()
            .filter_map(|site| {
                let text = std::fs::read_to_string(&site.file).ok()?;
                Some((site, text.get(site.span.clone())?.to_string()))
            })
            .collect()
    }

    fn record_script(&mut self, fighter: &str, script: String, site: ScriptSite) {
        let fighter = normalize_fighter(fighter);
        if let Some(conflict) = self
            .conflicts
            .iter_mut()
            .find(|conflict| conflict.fighter == fighter && conflict.script == script)
        {
            if !conflict.sites.contains(&site) {
                conflict.sites.push(site);
            }
            return;
        }

        let existing = self
            .fighters
            .entry(fighter.clone())
            .or_default()
            .scripts
            .remove(&script);
        if let Some(existing) = existing {
            self.conflicts.push(ScriptConflict {
                fighter,
                script,
                sites: vec![existing, site],
            });
        } else {
            self.fighters
                .get_mut(&fighter)
                .expect("record_script inserted the fighter above")
                .scripts
                .insert(script, site);
        }
    }
}

/// The nearest source-directory scope that unambiguously owns `file`.
///
/// `mod.rs` commonly declares an agent several folders above the individual ACMD files. Walking
/// ancestors keeps that ordinary module layout working while still refusing a nearer scope that
/// declares more than one fighter.
fn fighter_for_file(
    file: &Path,
    root: &Path,
    fighter_scopes: &HashMap<PathBuf, Vec<String>>,
) -> Option<String> {
    // A module written as a single file (`pacman.rs`) is its own scope.
    if let Some(names) = fighter_scopes.get(file) {
        if !names.is_empty() {
            return (names.len() == 1).then(|| names[0].clone());
        }
    }
    let mut directory = file.parent()?;
    loop {
        if let Some(names) = fighter_scopes.get(directory) {
            if names.len() == 1 {
                return Some(names[0].clone());
            }
            // Fighter kinds never contain `_`; `falco_arwing` is an article. A folder whose
            // files declare a fighter's articles beside it (`specials.rs` registers the
            // `falco_cannon` shots, `misc.rs` the `falco_arwing` entry) still holds that
            // fighter's own scripts, and one declaring only articles doesn't own its other
            // files at all: keep walking to the module that names the fighter. A folder
            // named for the mod (`emperte/`) rather than the fighter is then still Falco's.
            // `Agent::new("fighter")` / `("weapon")` is smashline's agent for every fighter or
            // weapon: its registrations name it as their receiver, and it owns no module (King
            // Boo's `status/` holds one beside the statuses it installs on `dedede`).
            let generic = |name: &str| matches!(name, "fighter" | "weapon");
            let fighters: Vec<&String> =
                names.iter().filter(|name| !name.contains('_') && !generic(name)).collect();
            match fighters.as_slice() {
                [] => {}
                [fighter] if names.iter().all(|name| {
                    *name == **fighter || name.starts_with(&format!("{fighter}_")) || generic(name)
                }) =>
                {
                    return Some((*fighter).clone());
                }
                _ => return None,
            }
        }
        if directory == root {
            return None;
        }
        directory = directory.parent()?;
    }
}

/// Modules handed an agent: `(module path on disk, fighter)` for each `path::to::f(agent)` call
/// in `text` whose argument is an `Agent::new("…")`, directly or through a `let` binding.
///
/// The module path is given both as a directory (`…/pacman`) and as a file (`…/pacman.rs`),
/// since either layout declares the same module; [`fighter_for_file`] looks for both.
fn scan_install_routes(file: &Path, text: &str) -> Vec<(PathBuf, String)> {
    let text = &without_comments(text);
    let mut bound: HashMap<String, String> = HashMap::new();
    let mut search = 0;
    while let Some(rel) = text[search..].find("Agent::new(") {
        let at = search + rel;
        search = at + "Agent::new(".len();
        let rest = text[search..].trim_start();
        let Some(name) = rest
            .strip_prefix('"')
            .and_then(|r| r.find('"').map(|end| r[..end].to_string()))
        else {
            continue;
        };
        // The statement this constructor sits in: `let [mut] VAR = [&mut] [path::]Agent::new(…`.
        let stmt_start = text[..at].rfind([';', '{', '}']).map_or(0, |n| n + 1);
        let head = text[stmt_start..at].trim();
        if let Some(binding) = head.strip_prefix("let ") {
            let binding = binding.trim_start().strip_prefix("mut ").unwrap_or(binding).trim_start();
            let end = ident_end(binding, 0);
            if end > 0 && binding[end..].trim_start().starts_with('=') {
                bound.insert(binding[..end].to_string(), normalize_fighter(&name));
            }
        }
    }
    let module_dir = match file.file_name().and_then(|n| n.to_str()) {
        Some("mod.rs" | "lib.rs" | "main.rs") => file.parent().map(Path::to_path_buf),
        _ => file
            .parent()
            .zip(file.file_stem())
            .map(|(parent, stem)| parent.join(stem)),
    };
    let Some(module_dir) = module_dir else {
        return Vec::new();
    };
    let crate_dir = file.ancestors().skip(1).find(|dir| {
        dir.join("lib.rs").is_file() || dir.join("main.rs").is_file()
    });
    let mut routes = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < text.len() {
        if !text.is_char_boundary(i) || !is_ident_byte(bytes[i]) || (i > 0 && is_ident_byte(bytes[i - 1])) {
            i += 1;
            continue;
        }
        // A path `a::b::f(`, read segment by segment.
        let mut segments: Vec<&str> = Vec::new();
        let mut at = i;
        loop {
            let end = ident_end(text, at);
            if end == at {
                break;
            }
            segments.push(&text[at..end]);
            at = end;
            if text[at..].starts_with("::") {
                at += 2;
                continue;
            }
            break;
        }
        if segments.len() < 2 || bytes.get(at) != Some(&b'(') {
            i = at.max(i + 1);
            continue;
        }
        let Some((args, close)) = split_call_args(text, at + 1, text.len()) else {
            i = at + 1;
            continue;
        };
        let fighter = args.iter().find_map(|span| {
            let arg = text[span.clone()].trim();
            let arg = arg.strip_prefix("&mut").unwrap_or(arg).trim_start();
            let arg = arg.strip_prefix('&').unwrap_or(arg).trim_start();
            if let Some(name) = bound.get(arg) {
                return Some(name.clone());
            }
            let inner = arg.split("Agent::new(").nth(1)?.trim_start().strip_prefix('"')?;
            Some(normalize_fighter(&inner[..inner.find('"')?]))
        });
        if let Some(fighter) = fighter {
            let modules = &segments[..segments.len() - 1];
            let mut dir = module_dir.clone();
            let mut ok = true;
            for (index, segment) in modules.iter().enumerate() {
                match *segment {
                    "self" if index == 0 => {}
                    "super" => match dir.parent() {
                        Some(parent) => dir = parent.to_path_buf(),
                        None => ok = false,
                    },
                    "crate" if index == 0 => match crate_dir {
                        Some(root) => dir = root.to_path_buf(),
                        None => ok = false,
                    },
                    // A type or an external crate (`smashline::…`) is not a module of ours.
                    s if s.chars().next().is_some_and(|c| c.is_ascii_uppercase()) => ok = false,
                    s => dir = dir.join(s),
                }
            }
            if ok && dir != module_dir {
                if dir.is_dir() || dir.with_extension("rs").is_file() {
                    routes.push((dir.with_extension("rs"), fighter.clone()));
                    routes.push((dir, fighter));
                }
            }
        }
        i = close + 1;
    }
    routes
}

/// Each `Agent::new("…")` in `text`, with whether it is narrowed to costume slots:
/// `.set_costume(…)` in the constructor's own chain, or called later on the variable it was
/// bound to, directly (`agent.set_costume(…)`) or through a rebinding (`let agent =
/// agent.set_costume(…);`, Empoleon's spelling). `common` hooks every fighter and is skipped.
///
/// A costume list handed in through a function parameter is not followed; the agent then
/// reads as ungated, which keeps the fighter's own slots credited, as before.
fn scan_agent_gating(text: &str) -> Vec<(String, bool)> {
    let text = &without_comments(text);
    let boundaries = statement_boundaries(text);
    // `(receiver, offset)` for each `.set_costume(` call.
    let gated_receivers: Vec<(&str, usize)> = {
        let mut receivers = Vec::new();
        let mut search = 0;
        while let Some(rel) = text[search..].find(".set_costume(") {
            let at = search + rel;
            search = at + 1;
            let head = text[statement_start(&boundaries, at)..at].trim_start();
            let head = head
                .strip_prefix("let ")
                .and_then(|rest| rest.split_once('=').map(|(_, value)| value))
                .unwrap_or(head)
                .trim_start();
            let head = head.strip_prefix("&mut").unwrap_or(head).trim_start();
            let head = head.strip_prefix('&').unwrap_or(head).trim_start();
            let head = head.strip_prefix('*').unwrap_or(head).trim_start();
            let end = ident_end(head, 0);
            if end > 0 {
                receivers.push((&head[..end], at));
            }
        }
        receivers
    };
    let mut found = Vec::new();
    let mut search = 0;
    while let Some(rel) = text[search..].find("Agent::new(") {
        let at = search + rel;
        search = at + "Agent::new(".len();
        let Some(fighter) = agent_literal(&text[at..]) else {
            continue;
        };
        if fighter.is_empty() || fighter == "common" {
            continue;
        }
        let end = chain_end(text, at);
        let mut gated = text[at..end].contains(".set_costume(");
        if !gated {
            let head = text[statement_start(&boundaries, at)..at].trim();
            if let Some(binding) = head.strip_prefix("let ") {
                let binding = binding.trim_start();
                let binding = binding.strip_prefix("mut ").unwrap_or(binding).trim_start();
                let name_end = ident_end(binding, 0);
                // Only later in the binding's own block: another function's `agent` is not it.
                let scope = at..block_end(text, at);
                gated = name_end > 0
                    && gated_receivers.iter().any(|(receiver, offset)| {
                        *receiver == &binding[..name_end] && scope.contains(offset)
                    });
            }
        }
        found.push((fighter, gated));
    }
    found
}

/// The `}` closing the block `start` sits in, or the end of `text`.
fn block_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = start;
    while i < text.len() {
        if let Some(next) = skip_trivia(text, i) {
            i = next;
            continue;
        }
        match bytes[i] {
            b'{' => depth += 1,
            b'}' if depth == 0 => return i,
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    text.len()
}

/// Where the expression starting at `start` ends: its `;`, the `}` closing the block it is
/// the tail of, or the end of `text`.
fn chain_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = start;
    while i < text.len() {
        if let Some(next) = skip_trivia(text, i) {
            i = next;
            continue;
        }
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            b'}' if depth == 0 => return i,
            b'}' => depth -= 1,
            b';' if depth == 0 => return i,
            _ => {}
        }
        i += 1;
    }
    text.len()
}

/// Visionary names fighters after the dump folder (`mario`); ACMD attributes and
/// `Agent::new` calls often spell the same character `fighter_mario`.
fn normalize_fighter(name: &str) -> String {
    let name = name.trim().to_ascii_lowercase();
    name.strip_prefix("fighter_").unwrap_or(&name).to_string()
}

/// The files a crate under `root` actually compiles, or `None` when `root` holds no crate.
///
/// Moveset folders routinely carry source the build never sees: a `srcOLD/` kept as a backup, a
/// `dist/` copy of a release's source, a `.zip` extracted beside the real tree, vanilla reference
/// dumps. All of it defines the same script names as the live code, so indexing it turned every
/// move into a "found in two places" conflict (Waluigi: all 96 scripts) or, worse, would let an
/// edit land in a file that is never built. Rust says exactly which files are in a crate — the
/// `mod` tree under `src/lib.rs` / `src/main.rs` — so that is what decides it.
///
/// A folder with no `Cargo.toml` anywhere (a loose set of ACMD files someone shared) keeps the
/// old behaviour: every `.rs` file counts.
fn compiled_files(root: &Path, texts: &HashMap<PathBuf, &str>) -> Option<HashSet<PathBuf>> {
    let mut crate_roots: Vec<PathBuf> = Vec::new();
    // Cargo knows exactly which crates the workspace builds, wherever their roots are. The
    // folder walk below is for when it cannot be asked: no cargo, or a loose folder of files.
    if let Some(project) = crate::cargo_project::CargoProject::discover(root) {
        crate_roots.extend(project.crate_roots().cloned());
    }
    if crate_roots.is_empty() {
        // The linked folder may be a crate's `src/` or `plugin/`, with the manifest above it.
        let mut manifests: Vec<PathBuf> = texts
            .keys()
            .filter_map(|file| {
                let mut dir = file.parent();
                while let Some(d) = dir {
                    if d.join("Cargo.toml").is_file() {
                        return Some(d.to_path_buf());
                    }
                    if d == root {
                        break;
                    }
                    dir = d.parent();
                }
                None
            })
            .collect();
        let mut ancestor = Some(root);
        for _ in 0..4 {
            let Some(dir) = ancestor else { break };
            let manifest = dir.join("Cargo.toml");
            if manifest.is_file() {
                // A plain package builds only itself: a crate nested inside it (a
                // `plugin(old)/` backup with its own manifest) is not a workspace member, and
                // cargo never compiles it.
                let toml = std::fs::read_to_string(&manifest).unwrap_or_default();
                if toml.contains("[package]") && !toml.contains("[workspace]") {
                    manifests.clear();
                }
                manifests.push(dir.to_path_buf());
                break;
            }
            ancestor = dir.parent();
        }
        manifests.sort();
        manifests.dedup();
        for manifest in &manifests {
            let toml = std::fs::read_to_string(manifest.join("Cargo.toml")).unwrap_or_default();
            // `[lib] path = "…"` relocates the crate root; take it when given.
            let custom = toml
                .lines()
                .map(str::trim)
                .filter(|line| line.starts_with("path"))
                .filter_map(|line| line.split_once('=').map(|(_, v)| v.trim().trim_matches('"')))
                .filter(|path| path.ends_with(".rs"))
                .map(|path| manifest.join(path))
                .collect::<Vec<_>>();
            crate_roots.extend(custom);
            for name in ["src/lib.rs", "src/main.rs"] {
                crate_roots.push(manifest.join(name));
            }
        }
    }
    crate_roots.retain(|path| path.is_file());
    if crate_roots.is_empty() {
        return None;
    }
    let mut reached: HashSet<PathBuf> = HashSet::new();
    let mut queue = crate_roots;
    while let Some(file) = queue.pop() {
        if !reached.insert(file.clone()) {
            continue;
        }
        let owned;
        let text = match texts.get(&file) {
            Some(text) => *text,
            None => {
                owned = std::fs::read_to_string(&file).unwrap_or_default();
                &owned
            }
        };
        let stem_dir = match file.file_name().and_then(|n| n.to_str()) {
            Some("mod.rs" | "lib.rs" | "main.rs") => file.parent().map(Path::to_path_buf),
            _ => file
                .parent()
                .zip(file.file_stem())
                .map(|(parent, stem)| parent.join(stem)),
        };
        let Some(stem_dir) = stem_dir else { continue };
        for child in module_declarations(text) {
            let base = child.inline_parents.iter().fold(stem_dir.clone(), |dir, m| dir.join(m));
            let candidates = match &child.path_attr {
                // `#[path]` is relative to the declaring file's directory (inline modules nest).
                Some(path) => vec![file
                    .parent()
                    .map(|p| child.inline_parents.iter().fold(p.to_path_buf(), |d, m| d.join(m)))
                    .unwrap_or_default()
                    .join(path)],
                None => vec![
                    base.join(format!("{}.rs", child.name)),
                    base.join(&child.name).join("mod.rs"),
                ],
            };
            if let Some(found) = candidates.into_iter().find(|p| p.is_file()) {
                queue.push(found);
            }
        }
    }
    Some(reached)
}

/// One `mod NAME;` found by [`module_declarations`].
struct ModuleDeclaration {
    name: String,
    /// Inline `mod a { mod b { … } }` blocks enclosing the declaration, outermost first.
    inline_parents: Vec<String>,
    /// A `#[path = "…"]` attribute on the declaration.
    path_attr: Option<String>,
}

/// Every out-of-line `mod NAME;` in `text`, ignoring comments and strings.
fn module_declarations(text: &str) -> Vec<ModuleDeclaration> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    // (module name, brace depth its body opened at)
    let mut inline: Vec<(String, usize)> = Vec::new();
    let mut depth = 0usize;
    let mut pending_path: Option<String> = None;
    let mut i = 0;
    while i < text.len() {
        if let Some(next) = skip_trivia(text, i) {
            i = next;
            continue;
        }
        match bytes[i] {
            b'{' => {
                depth += 1;
                i += 1;
                continue;
            }
            b'}' => {
                if inline.last().is_some_and(|(_, at)| *at == depth) {
                    inline.pop();
                }
                depth = depth.saturating_sub(1);
                i += 1;
                continue;
            }
            b'#' if text[i..].starts_with("#[path") => {
                let end = text[i..].find(']').map_or(text.len(), |n| i + n);
                pending_path = attr_string_value(&text[i..end], "path");
                i = end;
                continue;
            }
            _ => {}
        }
        if keyword_at(text, i, "mod") {
            let name_start = i + 3 + (text[i + 3..].len() - text[i + 3..].trim_start().len());
            let name_end = ident_end(text, name_start);
            if name_end > name_start {
                let name = text[name_start..name_end].to_string();
                let after = text[name_end..].trim_start();
                if after.starts_with(';') {
                    out.push(ModuleDeclaration {
                        name,
                        inline_parents: inline.iter().map(|(n, _)| n.clone()).collect(),
                        path_attr: pending_path.take(),
                    });
                } else if after.starts_with('{') {
                    pending_path = None;
                    inline.push((name, depth + 1));
                }
                i = name_end;
                continue;
            }
        }
        if !bytes[i].is_ascii_whitespace() && bytes[i] != b'#' && text.is_char_boundary(i) {
            // Any other token between an attribute and its item ends the attribute's reach.
            if bytes[i] == b';' {
                pending_path = None;
            }
        }
        i += 1;
    }
    out
}

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) -> Result<()> {
    // Deep enough for any real project layout, shallow enough that a symlink loop or an
    // accidentally-selected home directory cannot run away.
    if depth > 12 {
        return Ok(());
    }
    let entries = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || SKIPPED_DIRS.contains(&name.as_ref()) {
                continue;
            }
            collect_rust_files(&path, out, depth + 1)?;
        } else if kind.is_file()
            && path.extension().is_some_and(|e| e == "rs")
            && entry.metadata().is_ok_and(|m| m.len() <= MAX_SOURCE_BYTES)
        {
            out.push(path);
        }
    }
    Ok(())
}

/// One ACMD function found by [`scan_acmd_functions`].
#[derive(Debug, Clone)]
struct FoundFunction {
    name: String,
    /// Fighter named by a `#[acmd_script(agent = "…")]` attribute on this function, if any.
    fighter: Option<String>,
    /// Script named by that attribute's `script = "…"`. Older smash-script projects give the
    /// function whatever name they like and declare the script here.
    script: Option<String>,
    span: Range<usize>,
}

/// Every `unsafe extern "C" fn NAME(...) { ... }` in `text`, with its body span.
fn scan_acmd_functions(text: &str) -> Vec<FoundFunction> {
    let mut found = Vec::new();
    let mut search = 0;
    while let Some(rel) = text[search..].find("fn ") {
        let fn_kw = search + rel;
        search = fn_kw + 3;
        // Only take free functions declared the way ACMD scripts are; `fn` also appears in
        // trait bounds, closures' types, and ordinary helpers.
        let line_start = text[..fn_kw].rfind('\n').map_or(0, |n| n + 1);
        let prefix = text[line_start..fn_kw].trim();
        if !prefix.contains("extern") {
            continue;
        }
        let name_start = fn_kw + 3;
        let name_end = name_start
            + text[name_start..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(0);
        if name_end == name_start {
            continue;
        }
        let Some(body_start) = text[name_end..].find('{').map(|n| name_end + n) else {
            continue;
        };
        let Some(body_end) = match_brace(text, body_start) else {
            continue;
        };
        let (fighter, script) = attribute_values(text, line_start);
        found.push(FoundFunction {
            name: text[name_start..name_end].to_string(),
            fighter,
            script,
            span: line_start..body_end + 1,
        });
        search = body_end + 1;
    }
    found
}

/// Walk from an opening `{` to its match, ignoring braces inside strings and comments.
fn match_brace(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < text.len() {
        if let Some(next) = skip_trivia(text, i) {
            i = next;
            continue;
        }
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
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

/// `(agent, script)` from an `#[acmd_script(...)]` attribute above `line_start`.
fn attribute_values(text: &str, line_start: usize) -> (Option<String>, Option<String>) {
    // Attributes sit on the lines immediately above the declaration; scan back over them.
    let mut cursor = line_start;
    for _ in 0..8 {
        let prev_start = text[..cursor]
            .trim_end_matches('\n')
            .rfind('\n')
            .map_or(0, |n| n + 1);
        let line = text[prev_start..cursor].trim();
        if line.starts_with("#[") {
            let agent = attr_string_value(line, "agent");
            let script = attr_string_value(line, "script");
            if agent.is_some() || script.is_some() {
                return (agent, script);
            }
        } else if !line.is_empty() {
            return (None, None);
        }
        if prev_start == 0 {
            return (None, None);
        }
        cursor = prev_start;
    }
    (None, None)
}

/// `key = "value"` out of an attribute body.
///
/// The key has to be a whole word followed by `=`: `#[acmd_script(agent = "…")]` contains
/// the substring `script` inside its own name, and matching that read the agent as the
/// script name.
fn attr_string_value(line: &str, key: &str) -> Option<String> {
    let (span, quoted) = attr_value_span(line, key)?;
    quoted.then(|| line[span].to_string())
}

/// The span of `key`'s value inside an attribute line, and whether it was quoted.
///
/// A quoted span covers the text *between* the quotes, so splicing over it leaves them in
/// place. A bare one covers a path-shaped token — `category = ACMD_GAME`, and the `::` so a
/// fully-qualified spelling comes back whole rather than truncated at its first colon.
fn attr_value_span(line: &str, key: &str) -> Option<(Range<usize>, bool)> {
    let mut search = 0;
    while let Some(rel) = line[search..].find(key) {
        let at = search + rel;
        search = at + key.len();
        let preceded_by_word = line[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        if preceded_by_word {
            continue;
        }
        let after_key = line[search..].trim_start();
        let Some(rest) = after_key.strip_prefix('=') else {
            continue;
        };
        let value_at = line.len() - rest.trim_start().len();
        let rest = rest.trim_start();
        if let Some(quoted) = rest.strip_prefix('"') {
            let end = quoted.find('"')?;
            return Some((value_at + 1..value_at + 1 + end, true));
        }
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':'))
            .unwrap_or(rest.len());
        if end == 0 {
            continue;
        }
        return Some((value_at..value_at + end, false));
    }
    None
}

/// Fighter names a file declares, via `Agent::new("…")` or an `agent = "…"` attribute.
fn scan_fighter_names(text: &str) -> Vec<String> {
    // A commented-out `// Agent::new("fighter")` template line is not a declaration; reading it
    // gave Master Chief a fighter named `fighter` and split his scripts across two scopes.
    let text = &without_comments(text);
    let mut names: Vec<String> = Vec::new();
    let mut push = |name: String| {
        let name = normalize_fighter(&name);
        // `Agent::new("common")` hooks every fighter at once (opff, status lines). It is no
        // one's character, and counting it made a mod's own folder name two fighters.
        if !name.is_empty() && name != "common" && !names.contains(&name) {
            names.push(name);
        }
    };
    for marker in ["Agent::new(", "Agent::new_with_weapon("] {
        let mut search = 0;
        while let Some(rel) = text[search..].find(marker) {
            let at = search + rel + marker.len();
            search = at;
            let rest = text[at..].trim_start();
            if let Some(rest) = rest.strip_prefix('"') {
                if let Some(end) = rest.find('"') {
                    push(rest[..end].to_string());
                }
            }
        }
    }
    let mut search = 0;
    while let Some(rel) = text[search..].find("agent") {
        let at = search + rel;
        search = at + 5;
        let line_end = text[at..].find('\n').map_or(text.len(), |n| at + n);
        if let Some(value) = attr_string_value(&text[at..line_end], "agent") {
            push(value);
        }
    }
    names
}

/// One `agent.acmd("…", fn, …)`-style registration.
#[derive(Debug, Clone, PartialEq)]
struct Registration<'a> {
    script: &'a str,
    function: &'a str,
    /// The agent the call is made on, when the call itself says: a chain headed by
    /// `Agent::new("…")`, or a `let` binding of one. `None` for a parameter (`agent.acmd(…)`
    /// inside `fn install(agent: &mut Agent)`), whose agent only the caller knows.
    agent: Option<String>,
}

/// The function that wins each `(agent, script)` pair: smashline keeps one function per agent
/// and script, so a later registration replaces an earlier one — but only for the same agent.
fn last_registrations<'a>(
    registrations: &[Registration<'a>],
) -> HashMap<(Option<String>, &'a str), &'a str> {
    registrations
        .iter()
        .map(|registration| ((registration.agent.clone(), registration.script), registration.function))
        .collect()
}

/// Every `agent.acmd("…", fn, …)`-style registration in `text`, in document order, with the
/// agent each is made on.
fn scan_registrations(text: &str) -> Vec<Registration<'_>> {
    let mut out: Vec<(usize, &str, &str, Option<String>)> = Vec::new();
    let boundaries = statement_boundaries(text);
    let bindings = agent_bindings(text, &boundaries);
    for marker in [
        ".acmd(",
        ".game_acmd(",
        ".effect_acmd(",
        ".sound_acmd(",
        ".expression_acmd(",
        "install_acmd_script!(",
    ] {
        let mut search = 0;
        while search < text.len() {
            if !text.is_char_boundary(search) {
                search += 1;
                continue;
            }
            if let Some(next) = skip_trivia(text, search) {
                search = next;
                continue;
            }
            if !text[search..].starts_with(marker) {
                search += 1;
                continue;
            }
            let open = search + marker.len() - 1;
            search = open + 1;
            let Some((args, _)) = split_call_args(text, open + 1, text.len()) else {
                continue;
            };
            // (script, function) for `.acmd(…)`; the bang form leads with the agent.
            let (script, function, receiver) = match (marker, args.len()) {
                ("install_acmd_script!(", 3..) => (&args[1], &args[2], &text[args[0].clone()]),
                (_, 2..) => {
                    // The method chain this call hangs off, from its statement's start.
                    let dot = open + 1 - marker.len();
                    let start = statement_start(&boundaries, dot);
                    (&args[0], &args[1], &text[start..dot])
                }
                _ => continue,
            };
            let script = text[script.clone()].trim().trim_matches('"');
            let function = text[function.clone()].trim();
            if !script.is_empty() && is_identifier(function) {
                out.push((open, script, function, receiver_agent(receiver, &bindings)));
            }
        }
    }
    // Document order: when one agent registers a script twice, the later call replaces the
    // earlier one, and callers rely on reading them in the order the game does.
    out.sort_by_key(|(at, _, _, _)| *at);
    out.into_iter()
        .map(|(_, script, function, agent)| Registration { script, function, agent })
        .collect()
}

/// One `.status(Phase, KIND, f)` or `.on_start(f)` call, or one phase of a `noop_status!`.
struct StatusRegistration {
    /// Offset of the call, for its line number.
    at: usize,
    agent: Option<String>,
    phase: String,
    kind_expr: String,
    /// The function's own name: a path (`jackbox::jackbox_main`) keeps its last segment. Anything
    /// that is not a name (a closure) is kept as written.
    function: String,
    /// The module a path names the function in (`fly` for `fly::bullet_seed_end`), when it has one.
    module: Option<String>,
    /// A phase registered with `noop_status!`: there is no function to follow.
    noop: bool,
}

/// The module segment before a function's name in a path (`fly` in `fly::bullet_seed_end`), when
/// it is a plain module name rather than `self`, `super` or `crate`.
fn function_module(text: &str) -> Option<String> {
    let text = text.trim();
    let (module, _) = text.rsplit_once("::")?;
    let last = module.rsplit("::").next()?.trim();
    (!last.is_empty()
        && !matches!(last, "self" | "super" | "crate")
        && last.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    .then(|| last.to_string())
}

/// A path's last segment, unless the text is not a plain path (a closure), which stays as written.
fn function_name(text: &str) -> String {
    let text = text.trim();
    if text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':') {
        text.rsplit("::").next().unwrap_or(text).to_string()
    } else {
        text.to_string()
    }
}

/// Every `.status(..)` / `.on_start(..)` / `noop_status!(..)` registration in `text`, in document
/// order, with the agent each is made on when the call itself says (as [`scan_registrations`]).
fn scan_status_registrations(text: &str) -> Vec<StatusRegistration> {
    let mut out: Vec<StatusRegistration> = Vec::new();
    let boundaries = statement_boundaries(text);
    let bindings = agent_bindings(text, &boundaries);
    for marker in [".status(", ".on_start(", "noop_status!("] {
        let mut search = 0;
        while search < text.len() {
            if !text.is_char_boundary(search) {
                search += 1;
                continue;
            }
            if let Some(next) = skip_trivia(text, search) {
                search = next;
                continue;
            }
            if !text[search..].starts_with(marker) {
                search += 1;
                continue;
            }
            let at = search;
            let open = search + marker.len() - 1;
            search = open + 1;
            let Some((args, _)) = split_call_args(text, open + 1, text.len()) else {
                continue;
            };
            let receiver = &text[statement_start(&boundaries, at)..at];
            // `$agent.status($phase, $kind, …)` is the body of a `macro_rules!`: its invocations
            // are the registrations (`noop_status!` ones are read below), not the template.
            if args.iter().take(2).any(|arg| text[arg.clone()].trim_start().starts_with('$')) {
                continue;
            }
            match (marker, args.len()) {
                (".status(", 3..) => out.push(StatusRegistration {
                    at,
                    agent: receiver_agent(receiver, &bindings),
                    phase: function_name(&text[args[0].clone()]),
                    kind_expr: text[args[1].clone()].trim().to_string(),
                    function: function_name(&text[args[2].clone()]),
                    module: function_module(&text[args[2].clone()]),
                    noop: false,
                }),
                (".on_start(", 1) => out.push(StatusRegistration {
                    at,
                    agent: receiver_agent(receiver, &bindings),
                    phase: "Start".to_string(),
                    kind_expr: String::new(),
                    function: function_name(&text[args[0].clone()]),
                    module: function_module(&text[args[0].clone()]),
                    noop: false,
                }),
                ("noop_status!(", 3..) => {
                    let agent = receiver_agent(&text[args[0].clone()], &bindings);
                    let kind_expr = text[args[1].clone()].trim().to_string();
                    let phases = text[args[2].clone()].trim().trim_start_matches('[').trim_end_matches(']');
                    for phase in phases.split(',').map(str::trim).filter(|phase| !phase.is_empty()) {
                        out.push(StatusRegistration {
                            at,
                            agent: agent.clone(),
                            phase: function_name(phase),
                            kind_expr: kind_expr.clone(),
                            function: String::new(),
                            module: None,
                            noop: true,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    out.sort_by_key(|registration| registration.at);
    out
}

/// Offsets of every `;`, `{` and `}` outside strings and comments, ascending.
fn statement_boundaries(text: &str) -> Vec<usize> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if let Some(next) = skip_trivia(text, i) {
            i = next;
            continue;
        }
        if matches!(text.as_bytes()[i], b';' | b'{' | b'}') {
            found.push(i);
        }
        i += 1;
    }
    found
}

/// Where the statement containing `at` starts: just past the last boundary before it.
fn statement_start(boundaries: &[usize], at: usize) -> usize {
    let before = boundaries.partition_point(|&b| b < at);
    before.checked_sub(1).map_or(0, |i| boundaries[i] + 1)
}

/// The fighter named by the first `Agent::new("…")` in `text`, normalized.
fn agent_literal(text: &str) -> Option<String> {
    let rest = text.split("Agent::new(").nth(1)?.trim_start().strip_prefix('"')?;
    Some(normalize_fighter(&rest[..rest.find('"')?]))
}

/// `let [mut] NAME = [&mut] [path::]Agent::new("…")` bindings in `text`: name → fighter.
fn agent_bindings(text: &str, boundaries: &[usize]) -> HashMap<String, String> {
    let mut bound = HashMap::new();
    let mut search = 0;
    while let Some(rel) = text[search..].find("Agent::new(") {
        let at = search + rel;
        search = at + "Agent::new(".len();
        let head = text[statement_start(boundaries, at)..at].trim();
        let Some(binding) = head.strip_prefix("let ") else {
            continue;
        };
        let binding = binding.trim_start();
        let binding = binding.strip_prefix("mut ").unwrap_or(binding).trim_start();
        let end = ident_end(binding, 0);
        if end > 0 && binding[end..].trim_start().starts_with('=') {
            if let Some(fighter) = agent_literal(&text[at..]) {
                bound.insert(binding[..end].to_string(), fighter);
            }
        }
    }
    bound
}

/// The agent a registration's receiver names: an `Agent::new("…")` at the head of the chain, or
/// a variable bound to one. A parameter or anything else is `None`.
fn receiver_agent(receiver: &str, bindings: &HashMap<String, String>) -> Option<String> {
    let receiver = receiver.trim_start();
    if receiver.contains("Agent::new(") {
        return agent_literal(receiver);
    }
    let receiver = receiver.strip_prefix("&mut").unwrap_or(receiver).trim_start();
    let receiver = receiver.strip_prefix('&').unwrap_or(receiver).trim_start();
    let receiver = receiver.strip_prefix('*').unwrap_or(receiver).trim_start();
    let end = ident_end(receiver, 0);
    bindings.get(&receiver[..end]).cloned()
}

fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !value.starts_with(|c: char| c.is_ascii_digit())
}

// ── Scripts that forward to a shared implementation ───────────────────────────

/// Any free function or method with a body, found by [`scan_plain_functions`].
#[derive(Debug, Clone)]
struct PlainFunction {
    name: String,
    /// `(name, type)` per parameter, `self` included as `("self", "Self")`. `None` when the
    /// list holds something this reader cannot split, such as a pattern or a generic type
    /// with a comma, because a misnumbered parameter would substitute the wrong argument.
    params: Option<Vec<(String, String)>>,
    /// From the declaration's line start through its closing brace.
    span: Range<usize>,
    /// The body, `{` through `}` inclusive.
    body: Range<usize>,
}

/// `Type::method` → that method's `match self` arms, or `None` when the method is not a
/// trivial match (or is defined twice).
type MethodTable = HashMap<(String, String), Option<Vec<MatchArm>>>;

/// One `A | B => value` arm: the last path segment of each pattern, and the value text.
type MatchArm = (Vec<String>, String);

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The end of the identifier starting at `at`.
fn ident_end(text: &str, at: usize) -> usize {
    at + text[at..]
        .bytes()
        .position(|b| !is_ident_byte(b))
        .unwrap_or(text.len() - at)
}

/// Whether `text[at..]` begins the keyword `word` as a whole token.
fn keyword_at(text: &str, at: usize, word: &str) -> bool {
    let bytes = text.as_bytes();
    text.is_char_boundary(at)
        && text[at..].starts_with(word)
        && (at == 0 || !is_ident_byte(bytes[at - 1]))
        && bytes.get(at + word.len()).is_some_and(|b| !is_ident_byte(*b))
}

/// The span (declaration line start through closing brace) of the one function named `name` in
/// `text`. `None` when there is none, or more than one, since which one a call reaches would then
/// depend on module scoping this does not model.
pub(crate) fn function_item(text: &str, name: &str) -> Option<Range<usize>> {
    let mut found = scan_plain_functions(text).into_iter().filter(|f| f.name == name);
    let first = found.next()?;
    found.next().is_none().then_some(first.span)
}

/// Every `fn NAME(…) … { … }` in `text` with a body, nested ones included.
fn scan_plain_functions(text: &str) -> Vec<PlainFunction> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if let Some(next) = skip_trivia(text, i) {
            i = next;
            continue;
        }
        if !keyword_at(text, i, "fn") {
            i += 1;
            continue;
        }
        let fn_kw = i;
        let name_start = i + 2 + text[i + 2..].len() - text[i + 2..].trim_start().len();
        let name_end = ident_end(text, name_start);
        i = name_end.max(fn_kw + 2);
        if name_end == name_start {
            continue;
        }
        let Some(paren) = text[name_end..].find('(').map(|n| name_end + n) else {
            continue;
        };
        // Only generics may sit between the name and its parameter list.
        let between = text[name_end..paren].trim();
        if !(between.is_empty() || between.starts_with('<')) {
            continue;
        }
        let Some((args, close)) = split_call_args(text, paren + 1, text.len()) else {
            continue;
        };
        // The body, unless this is a bodiless declaration (`fn x();` in a trait).
        let mut j = close + 1;
        let mut open = None;
        while j < text.len() {
            if let Some(next) = skip_trivia(text, j) {
                j = next;
                continue;
            }
            match bytes[j] {
                b'{' => {
                    open = Some(j);
                    break;
                }
                b';' => break,
                _ => j += 1,
            }
        }
        let Some(open) = open else {
            continue;
        };
        let Some(close_brace) = match_brace(text, open) else {
            continue;
        };
        let params = args
            .iter()
            .map(|span| {
                let param = text[span.clone()].trim();
                if param.trim_start_matches('&').trim_start_matches("mut ").trim() == "self"
                    || param.ends_with("self") && !param.contains(':')
                {
                    return Some(("self".to_string(), "Self".to_string()));
                }
                let (name, ty) = param.split_once(':')?;
                let name = name.trim().trim_start_matches("mut ").trim();
                is_identifier(name).then(|| (name.to_string(), ty.trim().to_string()))
            })
            .collect::<Option<Vec<_>>>();
        let line_start = text[..fn_kw].rfind('\n').map_or(0, |n| n + 1);
        found.push(PlainFunction {
            name: text[name_start..name_end].to_string(),
            params,
            span: line_start..close_brace + 1,
            body: open..close_brace + 1,
        });
    }
    found
}

/// Every `impl … { … }` block's braces, with the type's last path segment for an inherent
/// impl. A trait impl carries `None`: its methods are neither free functions nor what this
/// reader resolves a `param.method()` through.
fn scan_impl_blocks(text: &str) -> Vec<(Option<String>, Range<usize>)> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if let Some(next) = skip_trivia(text, i) {
            i = next;
            continue;
        }
        if !keyword_at(text, i, "impl") {
            i += 1;
            continue;
        }
        i += 4;
        let Some(open) = text[i..].find(['{', ';']).map(|n| i + n) else {
            break;
        };
        if text.as_bytes()[open] != b'{' {
            continue;
        }
        let header = text[i..open].trim();
        // `impl<T> Type<T>` leads with its own generics.
        let header = if header.starts_with('<') {
            header.find('>').map_or("", |n| header[n + 1..].trim())
        } else {
            header
        };
        let inherent = !(header.contains(" for ") || header.contains(" where "));
        let ty = type_name(header.split('<').next().unwrap_or(""));
        if let Some(close) = match_brace(text, open) {
            found.push(((inherent && !ty.is_empty()).then_some(ty), open..close + 1));
        }
    }
    found
}

/// The bare type name out of a parameter's type: `&mut crate::Facing` → `Facing`.
fn type_name(ty: &str) -> String {
    let ty = ty.trim().trim_start_matches('&').trim();
    let ty = ty.strip_prefix("mut ").unwrap_or(ty).trim();
    let ty = ty.split('<').next().unwrap_or("");
    ty.rsplit("::").next().unwrap_or("").trim().to_string()
}

/// `text` with every comment blanked out, strings and everything else kept.
pub(crate) fn without_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with("//") || text[i..].starts_with("/*") {
            i = skip_trivia(text, i).unwrap_or(text.len());
            out.push(' ');
            continue;
        }
        if let Some(next) = skip_trivia(text, i) {
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

/// Split `text` at every top-level `separator`, ignoring nested brackets, strings and
/// comments. Pieces are trimmed; empty ones (a trailing comma) are dropped.
fn split_top_level(text: &str, separator: u8) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut pieces = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    let mut i = 0;
    while i < text.len() {
        if let Some(next) = skip_trivia(text, i) {
            i = next;
            continue;
        }
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b if b == separator && depth == 0 => {
                pieces.push(text[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    pieces.push(text[start..].trim());
    pieces.retain(|piece| !piece.is_empty());
    pieces
}

/// A numeric literal as ACMD source spells one: `3`, `-28.4`, `1.`. No suffixes.
fn is_number(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    !whole.is_empty()
        && whole.bytes().all(|b| b.is_ascii_digit())
        && fraction.bytes().all(|b| b.is_ascii_digit())
}

/// A value safe to paste in place of a name: a number, a bool, or a (dereferenced) constant
/// path such as `*ATTACK_LR_CHECK_F`. Anything with an operator or a call is left alone.
fn is_simple_value(text: &str) -> bool {
    let text = text.trim();
    if is_number(text) || text == "true" || text == "false" {
        return true;
    }
    let path = text.strip_prefix('*').unwrap_or(text);
    !path.is_empty() && path.split("::").all(is_identifier)
}

/// The arms of a `match` block's interior.
fn parse_arms(text: &str) -> Option<Vec<MatchArm>> {
    split_top_level(text, b',')
        .into_iter()
        .map(|arm| {
            let (pattern, value) = arm.split_once("=>")?;
            if pattern.contains(" if ") {
                return None; // a guard decides at run time
            }
            let patterns = pattern
                .split('|')
                .map(|alternative| {
                    let alternative = alternative.trim();
                    alternative.rsplit("::").next().unwrap_or(alternative).trim().to_string()
                })
                .collect();
            Some((patterns, value.trim().to_string()))
        })
        .collect()
}

/// The arms of a method whose whole body is `match self { … }`, optionally inside
/// `unsafe { … }`. Every arm's value must be a [simple value](is_simple_value).
fn parse_self_match(body: &str) -> Option<Vec<MatchArm>> {
    let body = without_comments(body);
    let mut inner = body.trim().strip_prefix('{')?.strip_suffix('}')?.trim();
    while let Some(rest) = inner.strip_prefix("unsafe") {
        inner = rest.trim().strip_prefix('{')?.strip_suffix('}')?.trim();
    }
    let rest = inner.strip_prefix("match")?.trim_start();
    let rest = rest.strip_prefix("self")?.trim_start();
    if !rest.starts_with('{') {
        return None;
    }
    let close = match_brace(rest, 0)?;
    if !rest[close + 1..].trim().is_empty() {
        return None;
    }
    let arms = parse_arms(&rest[1..close])?;
    arms.iter()
        .all(|(_, value)| is_simple_value(value))
        .then_some(arms)
}

/// The value of the arm matching `key`, falling back to a `_` arm.
fn arm_for<'a>(arms: &'a [MatchArm], key: &str) -> Option<&'a str> {
    arms.iter()
        .find(|(patterns, _)| patterns.iter().any(|p| p == key))
        .or_else(|| arms.iter().find(|(patterns, _)| patterns.iter().any(|p| p == "_")))
        .map(|(_, value)| value.as_str())
}

/// A registered function whose body is one call to another function.
#[derive(Debug, Clone)]
struct Forward {
    /// The called function's name, its last path segment.
    callee: String,
    /// Offset of the wrapper's body `{` within its item.
    open: usize,
    /// The call's arguments, verbatim.
    args: Vec<String>,
    /// The wrapper's own parameter names, to recognise pass-through arguments.
    params: Vec<String>,
}

/// `fn game_x(agent) { shared_impl(agent, Facing::Reverse) }` → the call it forwards to.
fn forwarding_call(item: &str) -> Option<Forward> {
    let function = scan_plain_functions(item).into_iter().next()?;
    let inner = without_comments(&item[function.body.start + 1..function.body.end - 1]);
    let inner = inner.trim();
    let inner = inner.strip_suffix(';').unwrap_or(inner).trim_end();
    let path_end = inner
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':'))
        .unwrap_or(inner.len());
    let callee = inner[..path_end].rsplit("::").next()?;
    if !is_identifier(callee) || !inner[path_end..].starts_with('(') {
        return None;
    }
    let (args, close) = split_call_args(inner, path_end + 1, inner.len())?;
    if close + 1 != inner.len() {
        return None;
    }
    Some(Forward {
        callee: callee.to_string(),
        open: function.body.start,
        args: args.iter().map(|span| inner[span.clone()].to_string()).collect(),
        params: function
            .params
            .unwrap_or_default()
            .into_iter()
            .map(|(name, _)| name)
            .collect(),
    })
}

/// A shared body as one caller sees it.
struct Resolution {
    body: String,
    substituted: bool,
    unresolved: Vec<String>,
}

/// What one callee parameter is for this caller.
enum ParamValue {
    /// An enum variant (`Facing::Reverse` → `Reverse`) of the parameter's type.
    Variant { variant: String, ty: String },
    /// A literal, pasted wherever the parameter is read.
    Literal(String),
    /// Anything else: an expression the index cannot evaluate.
    Dynamic,
}

/// Substitute a caller's arguments into a shared body, `{` through `}`.
///
/// Three shapes resolve, all of them statically certain:
/// - `let (a, b) = match param { Variant => (x, y), … };`, which binds `a`/`b` for the rest of
///   the body and is itself removed;
/// - `param.method()`, where `method` is a [trivial `match self`](parse_self_match);
/// - a literal argument, read directly.
///
/// Constant products the substitution exposes (`28.4 * -1.0`) are then folded, since the
/// readers downstream expect a number in those slots. Every other read of a parameter is left
/// as written and reported, never guessed.
fn resolve_shared_body(
    body: &str,
    params: &[(String, String)],
    args: &[String],
    wrapper_params: &[String],
    methods: &MethodTable,
) -> Resolution {
    let mut unresolved = Vec::new();
    let callee_params: Vec<&(String, String)> =
        params.iter().filter(|(name, _)| name != "self").collect();
    if callee_params.len() != args.len() {
        unresolved.push(format!(
            "passes {} argument(s) to a function taking {}",
            args.len(),
            callee_params.len()
        ));
        return Resolution {
            body: body.to_string(),
            substituted: false,
            unresolved,
        };
    }
    let mut values: HashMap<&str, ParamValue> = HashMap::new();
    for ((name, ty), arg) in callee_params.into_iter().zip(args) {
        let arg = arg.trim();
        if is_identifier(arg) && wrapper_params.iter().any(|param| param == arg) {
            continue; // the agent, passed straight through
        }
        let value = if is_number(arg) || arg == "true" || arg == "false" {
            ParamValue::Literal(arg.to_string())
        } else if arg.contains("::") && arg.split("::").all(is_identifier) {
            ParamValue::Variant {
                variant: arg.rsplit("::").next().unwrap_or(arg).to_string(),
                ty: type_name(ty),
            }
        } else {
            ParamValue::Dynamic
        };
        values.insert(name.as_str(), value);
    }

    let mut edits: Vec<Replacement> = Vec::new();
    let mut removed: Vec<Range<usize>> = Vec::new();
    let mut bindings: HashMap<String, String> = HashMap::new();
    let mut unbound: Vec<String> = Vec::new();

    // `let PATTERN = match PARAM { … };`
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < body.len() {
        if let Some(next) = skip_trivia(body, i) {
            i = next;
            continue;
        }
        if !keyword_at(body, i, "let") {
            i += 1;
            continue;
        }
        let let_at = i;
        i += 3;
        let Some(semi) = statement_end(body, let_at) else {
            continue;
        };
        let Some(binding) = parse_let_match(&body[let_at..semi]) else {
            continue;
        };
        let Some(value) = values.get(binding.scrutinee.as_str()) else {
            continue;
        };
        let key = match value {
            ParamValue::Variant { variant, .. } => variant.clone(),
            ParamValue::Literal(literal) => literal.clone(),
            ParamValue::Dynamic => {
                unbound.extend(binding.names);
                continue;
            }
        };
        let chosen = binding
            .arms
            .as_deref()
            .and_then(|arms| arm_for(arms, &key))
            .map(|value| {
                if binding.names.len() == 1 {
                    Some(vec![value.to_string()])
                } else {
                    let inner = value.trim().strip_prefix('(')?.strip_suffix(')')?;
                    let parts = split_top_level(inner, b',');
                    (parts.len() == binding.names.len())
                        .then(|| parts.into_iter().map(str::to_string).collect())
                }
            });
        let Some(Some(parts)) = chosen else {
            unresolved.push(format!(
                "`let {} = match {} {{ … }}` has no arm this reader can pick for {key}",
                binding.pattern, binding.scrutinee
            ));
            unbound.extend(binding.names);
            continue;
        };
        for (name, part) in binding.names.iter().zip(parts) {
            if is_simple_value(&part) {
                bindings.insert(name.clone(), part);
            } else {
                unresolved.push(format!("{name} = `{part}` is not a literal"));
                unbound.push(name.clone());
            }
        }
        // Drop the whole line when the statement owns it, so no blank husk is left behind.
        let line_start = body[..let_at].rfind('\n').map_or(0, |n| n + 1);
        let start = if body[line_start..let_at].trim().is_empty() {
            line_start
        } else {
            let_at
        };
        let mut end = semi + 1;
        let rest = &body[end..];
        let trailing = rest.len() - rest.trim_start_matches([' ', '\t', '\r']).len();
        if rest[trailing..].starts_with('\n') {
            end += trailing + 1;
        }
        removed.push(start..end);
        edits.push(Replacement {
            span: start..end,
            value: String::new(),
        });
        i = end;
    }

    // Reads of a parameter or of a binding made above.
    let mut i = 0;
    while i < body.len() {
        if let Some(next) = skip_trivia(body, i) {
            i = next;
            continue;
        }
        if !body.is_char_boundary(i)
            || !is_ident_byte(bytes[i])
            || bytes[i].is_ascii_digit()
            || (i > 0 && is_ident_byte(bytes[i - 1]))
        {
            i += 1;
            continue;
        }
        let start = i;
        let end = ident_end(body, start);
        i = end;
        if removed.iter().any(|range| range.contains(&start)) {
            continue;
        }
        let word = &body[start..end];
        let before = body[..start].trim_end();
        if before.ends_with('.') || before.ends_with("::") {
            continue; // a field, a method or a path segment of the same name
        }
        let after = body[end..].trim_start();
        let is_read = !(after.starts_with('(')
            || after.starts_with('!')
            || after.starts_with("::")
            || (after.starts_with(':') && before.ends_with(['{', ','])));
        if let Some(ParamValue::Variant { variant, ty }) = values.get(word) {
            // `param.method()`
            let Some(call) = method_call(body, end) else {
                continue; // any other read is reported below
            };
            let found = methods
                .get(&(ty.clone(), call.method.clone()))
                .map(|arms| arms.as_deref().and_then(|arms| arm_for(arms, variant)));
            match found {
                Some(Some(value)) => {
                    edits.push(Replacement {
                        span: start..call.end,
                        value: value.to_string(),
                    });
                    i = call.end;
                }
                Some(None) => unresolved.push(format!(
                    "{word}.{}() is not a plain `match self` of literals",
                    call.method
                )),
                None => unresolved.push(format!(
                    "{word}.{}() is not a method of {ty} in the project",
                    call.method
                )),
            }
            continue;
        }
        if !is_read {
            continue;
        }
        let value = match values.get(word) {
            Some(ParamValue::Literal(literal)) => Some(literal.clone()),
            _ => bindings.get(word).cloned(),
        };
        if let Some(value) = value {
            edits.push(Replacement {
                span: start..end,
                value,
            });
        }
    }

    let substituted = !edits.is_empty();
    let mut resolved = apply(body, edits);
    if substituted {
        resolved = fold_constant_products(&resolved);
    }

    // Whatever still reads a parameter, or a binding that could not be made, is reported.
    let mut leftover: Vec<&str> = values
        .iter()
        .filter(|(_, value)| !matches!(value, ParamValue::Literal(_)))
        .map(|(name, _)| *name)
        .collect();
    leftover.extend(unbound.iter().map(String::as_str));
    let mut reported_lines: Vec<String> = Vec::new();
    let out = resolved.as_bytes();
    let mut i = 0;
    while i < resolved.len() {
        if let Some(next) = skip_trivia(&resolved, i) {
            i = next;
            continue;
        }
        if !is_ident_byte(out[i]) || (i > 0 && is_ident_byte(out[i - 1])) {
            i += 1;
            continue;
        }
        let end = ident_end(&resolved, i);
        let word = &resolved[i..end];
        let field = resolved[..i].trim_end().ends_with('.');
        if !field && leftover.contains(&word) {
            let line_start = resolved[..i].rfind('\n').map_or(0, |n| n + 1);
            let line_end = resolved[i..].find('\n').map_or(resolved.len(), |n| i + n);
            let line = resolved[line_start..line_end].trim().to_string();
            if !reported_lines.contains(&line) {
                unresolved.push(format!("`{line}` depends on {word}, which is not resolved"));
                reported_lines.push(line);
            }
        }
        i = end;
    }

    Resolution {
        body: resolved,
        substituted,
        unresolved,
    }
}

/// The offset of the `;` ending the statement that starts at `start`, at bracket depth 0.
pub(crate) fn statement_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = start;
    while i < text.len() {
        if let Some(next) = skip_trivia(text, i) {
            i = next;
            continue;
        }
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            b'}' => {
                if depth == 0 {
                    return None; // the block ended first
                }
                depth -= 1;
            }
            b';' if depth == 0 => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// `let (a, b) = match scrutinee { … }`, without its `;`.
struct LetMatch {
    pattern: String,
    names: Vec<String>,
    scrutinee: String,
    /// `None` when the arms could not be read.
    arms: Option<Vec<MatchArm>>,
}

fn parse_let_match(statement: &str) -> Option<LetMatch> {
    let statement = without_comments(statement);
    let rest = statement.trim().strip_prefix("let")?;
    let (pattern, rhs) = rest.split_once('=')?;
    if rhs.starts_with(['=', '>']) {
        return None;
    }
    let pattern = pattern.trim();
    // A type ascription (`let x: f32 = …`) is fine; only the names matter.
    let names_part = match pattern.strip_prefix('(') {
        Some(tuple) => tuple.strip_suffix(')')?,
        None => pattern.split(':').next()?,
    };
    let names = split_top_level(names_part, b',')
        .into_iter()
        .map(|name| {
            let name = name.strip_prefix("mut ").unwrap_or(name).trim();
            is_identifier(name).then(|| name.to_string())
        })
        .collect::<Option<Vec<_>>>()?;
    let rhs = rhs.trim().strip_prefix("match")?;
    if !rhs.starts_with(char::is_whitespace) {
        return None;
    }
    let rhs = rhs.trim_start();
    let scrutinee_end = ident_end(rhs, 0);
    let scrutinee = &rhs[..scrutinee_end];
    let block = rhs[scrutinee_end..].trim_start();
    if scrutinee.is_empty() || !block.starts_with('{') {
        return None;
    }
    let close = match_brace(block, 0)?;
    if !block[close + 1..].trim().is_empty() {
        return None;
    }
    Some(LetMatch {
        pattern: pattern.to_string(),
        names,
        scrutinee: scrutinee.to_string(),
        arms: parse_arms(&block[1..close]),
    })
}

/// `.method()` right after a receiver ending at `at`.
struct MethodCall {
    method: String,
    /// Just past the call's `)`.
    end: usize,
}

fn method_call(text: &str, at: usize) -> Option<MethodCall> {
    let rest = text[at..].strip_prefix('.')?;
    let name_end = ident_end(rest, 0);
    let method = &rest[..name_end];
    let call = rest[name_end..].strip_prefix('(')?;
    let close = call.len() - call.trim_start().len();
    if !call[close..].starts_with(')') || method.is_empty() {
        return None;
    }
    Some(MethodCall {
        method: method.to_string(),
        end: at + 1 + name_end + 1 + close + 1,
    })
}

/// Fold every `NUMBER * NUMBER` into its product: `28.4 * -1.0` → `-28.4`.
///
/// Only products whose surroundings cannot bind tighter are folded: an operand after `/`,
/// `*`, `%` or `.` belongs to that operation first, and a number followed by `.` is a method
/// receiver. A leading `-` counts as the operand's sign only where it cannot be subtraction.
fn fold_constant_products(text: &str) -> String {
    let mut text = text.to_string();
    let mut search = 0;
    'scan: while search < text.len() {
        let mut i = search;
        while i < text.len() {
            if let Some(next) = skip_trivia(&text, i) {
                i = next;
                continue;
            }
            if text.as_bytes()[i] == b'*' {
                if let Some((range, product)) = constant_product(&text, i) {
                    text.replace_range(range.clone(), &product);
                    search = range.start;
                    continue 'scan;
                }
            }
            i += 1;
        }
        break;
    }
    text
}

/// The span and folded value of a `NUMBER * NUMBER` whose `*` is at `star`.
fn constant_product(text: &str, star: usize) -> Option<(Range<usize>, String)> {
    let bytes = text.as_bytes();
    // Left operand, read backwards.
    let left_end = text[..star].trim_end().len();
    let mut left_start = left_end;
    while left_start > 0 && (bytes[left_start - 1].is_ascii_digit() || bytes[left_start - 1] == b'.')
    {
        left_start -= 1;
    }
    if left_start == left_end || !bytes[left_start].is_ascii_digit() {
        return None;
    }
    if left_start > 0 && is_ident_byte(bytes[left_start - 1]) {
        return None;
    }
    let before_sign = text[..left_start].trim_end();
    if before_sign.ends_with('-') {
        let ahead = text[..before_sign.len() - 1].trim_end();
        if ahead.is_empty() || ahead.ends_with(['(', ',', '=', '[', '{']) {
            left_start = before_sign.len() - 1;
        }
    }
    let ahead = text[..left_start].trim_end();
    if ahead.ends_with(['/', '*', '%', '.']) {
        return None;
    }
    // Right operand.
    let right_start = star + 1 + (text[star + 1..].len() - text[star + 1..].trim_start().len());
    let mut right_end = right_start;
    if bytes.get(right_end) == Some(&b'-') {
        right_end += 1;
    }
    while right_end < text.len() && (bytes[right_end].is_ascii_digit() || bytes[right_end] == b'.')
    {
        right_end += 1;
    }
    let left = &text[left_start..left_end];
    let right = &text[right_start..right_end];
    if !is_number(left) || !is_number(right) {
        return None;
    }
    let after = text[right_end..].trim_start();
    if right.ends_with('.')
        || after.starts_with('.')
        || bytes.get(right_end).is_some_and(|b| is_ident_byte(*b))
    {
        return None;
    }
    let product = if left.contains('.') || right.contains('.') {
        let value = left.parse::<f32>().ok()? * right.parse::<f32>().ok()?;
        if !value.is_finite() {
            return None;
        }
        let mut formatted = format!("{value}");
        if !formatted.contains('.') {
            formatted.push_str(".0");
        }
        formatted
    } else {
        left.parse::<i64>().ok()?.checked_mul(right.parse::<i64>().ok()?)?.to_string()
    };
    Some((left_start..right_end, product))
}

// ── Creating a script the project does not have ───────────────────────────────

/// A script written into the user's project, and how it was registered.
#[derive(Debug, Clone, PartialEq)]
pub struct CreatedScript {
    pub file: PathBuf,
    /// One sentence for the sync report: what was written, beside what, and installed how.
    /// Registration is the half that can silently do nothing, so it is always named.
    pub note: String,
}

/// How a project binds one Rust function to one ACMD script name.
#[derive(Debug, Clone, PartialEq)]
enum Binding {
    /// An `#[acmd_script(...)]` attribute line, verbatim.
    Attribute(String),
    /// An `agent.acmd("…", fn, …);` statement, verbatim, with the script-name and
    /// function-name spans *relative to that text* and the file offset just past it.
    Call {
        text: String,
        script: Range<usize>,
        function: Range<usize>,
        end: usize,
    },
    /// Nothing explicit — the function is reached by its conventional name alone.
    Convention,
}

/// A sibling script to copy a registration from, and where to put the new function.
#[derive(Debug, Clone)]
struct Anchor {
    file: PathBuf,
    /// Offset just past the sibling function, where the new one is inserted.
    after: usize,
    /// The sibling's ACMD script name. Its Rust function name is not kept: it is needed only to
    /// *find* the registration, and the new function's own name comes from the text being written.
    script: String,
    binding: Binding,
}

/// Write `source` into the user's project as `script_name`, registered the way its siblings are.
///
/// `source` is a whole `unsafe extern "C" fn …` block. It is expected to be the *mirror's* text
/// for this category rather than something regenerated from the IR: the effect emitter drops
/// lines it could not type, and creating a function that way would write a lossy copy of vanilla
/// into the user's project under their name. Copying the text and letting the ordinary value
/// sync edit it afterwards keeps creation under the same rule as every other write.
///
/// The caller must rebuild its [`SourceIndex`] afterwards. Every span in the anchor's file past
/// the insertion point has moved, and patching them by arithmetic across two insertions is the
/// kind of thing that works until it does not.
pub fn create_script(
    index: &SourceIndex,
    fighter: &str,
    script_name: &str,
    source: &str,
) -> Result<CreatedScript> {
    if let Some(conflict) = index.conflict(fighter, script_name) {
        bail!("{}", conflict.description());
    }
    if index.script(fighter, script_name).is_some() {
        bail!("{fighter}: the project already has a {script_name}");
    }
    let Some(anchor) = find_anchor(index, fighter, script_name) else {
        bail!(
            "{fighter}: the project has no script to put {script_name} beside — \
             it needs at least one ACMD function for this fighter first"
        );
    };
    let Some(function) = function_name_of(source) else {
        bail!("{script_name}: no `fn` to write — the source for it is not a function");
    };
    let text = std::fs::read_to_string(&anchor.file)
        .with_context(|| format!("reading {}", anchor.file.display()))?;
    if anchor.after > text.len() {
        bail!(
            "{} changed on disk since it was indexed — rescan and try again",
            anchor.file.display()
        );
    }
    let where_at = anchor
        .file
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();

    // Highest offset first, so an earlier insertion cannot move a later one's index.
    let mut inserts: Vec<(usize, String)> = Vec::new();
    // The new function is copied from the reference script, which spells everything the
    // canonical way; written into this file it takes the file's own spelling instead — a bare
    // `ATTACK(` in a project that glob-imports the macros, its agent name, its indentation.
    let style = crate::source_style::SourceStyle::detect(&text);
    let styled = style.apply(source.trim_end(), "");
    let body = styled.trim_end();
    let note = match &anchor.binding {
        Binding::Attribute(line) => {
            let attribute = rebuild_attribute(line, &anchor.script, script_name)?;
            inserts.push((anchor.after, format!("\n\n{attribute}\n{body}\n")));
            format!(
                "created {script_name} in {where_at}, after {}, with its own \
                 #[acmd_script] attribute",
                anchor.script
            )
        }
        Binding::Call {
            text: statement,
            script,
            function: function_span,
            end,
        } => {
            let mut registration = statement.clone();
            // Later span first, for the same reason the insertions are ordered.
            let (first, second) = if script.start < function_span.start {
                (function_span, script)
            } else {
                (script, function_span)
            };
            registration.replace_range(first.clone(), &function);
            registration.replace_range(second.clone(), script_name);
            inserts.push((*end, format!("\n{registration}")));
            inserts.push((anchor.after, format!("\n\n{body}\n")));
            format!(
                "created {script_name} in {where_at}, and registered it beside {}",
                anchor.script
            )
        }
        Binding::Convention => {
            inserts.push((anchor.after, format!("\n\n{body}\n")));
            format!(
                "created {script_name} in {where_at}, named the same conventional way as {} — \
                 which this project registers nowhere Visionary can see, so check it installs",
                anchor.script
            )
        }
    };

    // The surrounding text and the registration line come from this file already; only the
    // line endings of what joins them need to match it.
    if style.newline == "\r\n" {
        for (_, insert) in &mut inserts {
            *insert = insert.replace("\r\n", "\n").replace('\n', "\r\n");
        }
    }
    inserts.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    let mut updated = text;
    for (at, insert) in inserts {
        if at > updated.len() || !updated.is_char_boundary(at) {
            bail!(
                "{} changed on disk since it was indexed — rescan and try again",
                anchor.file.display()
            );
        }
        updated.insert_str(at, &insert);
    }
    std::fs::write(&anchor.file, &updated)
        .with_context(|| format!("writing {}", anchor.file.display()))?;
    Ok(CreatedScript {
        file: anchor.file,
        note,
    })
}

/// The sibling to write next to: this move's other categories first, then anything else.
///
/// The same move is preferred because its function is where a reader expects the new one to
/// appear. The fallback matters more than it looks: a project that defines the move in no
/// category at all still has *somewhere* for this fighter, and that is enough to know the file,
/// the fighter attribution and the registration style.
fn find_anchor(index: &SourceIndex, fighter: &str, script_name: &str) -> Option<Anchor> {
    let scripts = &index.fighters.get(&normalize_fighter(fighter))?.scripts;
    let mut order: Vec<String> = Vec::new();
    if let Some((_, suffix)) = script_name.split_once('_') {
        for prefix in crate::acmd::SCRIPT_PREFIXES {
            order.push(format!("{prefix}{suffix}"));
        }
    }
    let mut rest: Vec<String> = scripts.keys().cloned().collect();
    rest.sort();
    order.extend(rest);

    for name in order {
        if name == script_name {
            continue;
        }
        let Some(site) = scripts.get(&name) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&site.file) else {
            continue;
        };
        let Some(function) = text.get(site.span.clone()).and_then(function_name_of) else {
            continue;
        };
        let binding = if let Some(line) = attribute_line(&text, site.span.start) {
            Binding::Attribute(line)
        } else if let Some(call) = registration_statement(&text, &name, &function) {
            call
        } else {
            Binding::Convention
        };
        return Some(Anchor {
            file: site.file.clone(),
            after: site.span.end,
            script: name,
            binding,
        });
    }
    None
}

/// The name declared by the first `fn` in a function block.
fn function_name_of(source: &str) -> Option<String> {
    let at = source.find("fn ")? + 3;
    let end = at + source[at..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))?;
    (end > at).then(|| source[at..end].to_string())
}

/// The `#[acmd_script(...)]` line above a function declaration, verbatim.
fn attribute_line(text: &str, line_start: usize) -> Option<String> {
    let mut cursor = line_start;
    for _ in 0..8 {
        let prev_start = text[..cursor]
            .trim_end_matches('\n')
            .rfind('\n')
            .map_or(0, |n| n + 1);
        let line = text[prev_start..cursor].trim();
        if line.starts_with("#[") {
            if line.contains("acmd_script") {
                return Some(line.to_string());
            }
        } else if !line.is_empty() {
            return None;
        }
        if prev_start == 0 {
            return None;
        }
        cursor = prev_start;
    }
    None
}

/// The same attribute, respelled for `target` instead of `sibling`.
///
/// `script = "…"` is substituted when present; when it is absent the attribute names only the
/// agent and the function's own conventional name carries the binding, which is already true of
/// the text being written.
fn rebuild_attribute(line: &str, sibling: &str, target: &str) -> Result<String> {
    let mut out = line.to_string();
    if let Some((span, quoted)) = attr_value_span(line, "category") {
        if !quoted {
            let Some(token) = derive_category(sibling, &line[span.clone()], target) else {
                bail!(
                    "cannot tell what `category` a {target} should carry: {sibling} is \
                     declared `{}`, which is not the name its own category implies, so there \
                     is nothing here to derive the other one from",
                    &line[span]
                );
            };
            out.replace_range(span, &token);
        }
    }
    let script = attr_value_span(&out, "script").filter(|(_, quoted)| *quoted);
    if let Some((span, _)) = script {
        out.replace_range(span, target);
    }
    Ok(out)
}

/// The `category = …` token a `target` script should carry, read off the sibling's own.
///
/// Derives only when the sibling's token is exactly the `ACMD_<CATEGORY>` its *own* prefix
/// implies. There is no copy of the `#[acmd_script]` macro on this machine to check a spelling
/// against, so the project's file is the only oracle available — and it is an oracle only while
/// it agrees with itself. Anything else is refused rather than guessed, because a function
/// installed under the wrong category compiles and then replaces the wrong script.
fn derive_category(sibling_script: &str, token: &str, target_script: &str) -> Option<String> {
    let category_of = |script: &str| {
        crate::acmd::SCRIPT_PREFIXES
            .iter()
            .find(|prefix| script.starts_with(*prefix))
            .map(|prefix| prefix.trim_end_matches('_').to_ascii_uppercase())
    };
    let sibling = category_of(sibling_script)?;
    let target = category_of(target_script)?;
    (token.trim() == format!("ACMD_{sibling}")).then(|| format!("ACMD_{target}"))
}

/// The `agent.acmd("…", fn, …);` statement that installs `script`, ready to be respelled.
///
/// Only the general `.acmd(` form is matched, deliberately. `.game_acmd(` and `.effect_acmd(`
/// name the category in the *method*, so mirroring one for a different category would spell a
/// method this project may not have — and there is no smashline on this machine to ask. A
/// project registering that way falls through to [`Binding::Convention`], which writes the
/// function and says plainly that it could not be registered.
fn registration_statement(text: &str, script: &str, function: &str) -> Option<Binding> {
    const MARKER: &str = ".acmd(";
    let mut search = 0;
    while let Some(rel) = text[search..].find(MARKER) {
        let open = search + rel + MARKER.len() - 1;
        search = open + 1;
        let Some((args, close)) = split_call_args(text, open + 1, text.len()) else {
            continue;
        };
        if args.len() < 2 {
            continue;
        }
        // A literal, not a constant: the span arithmetic below steps over the quotes, and a
        // `const NAME` in that slot has none to step over.
        let named = &text[args[0].clone()];
        if !(named.starts_with('"') && named.ends_with('"') && named.len() >= 2) {
            continue;
        }
        if named.trim_matches('"') != script || text[args[1].clone()] != *function {
            continue;
        }
        let start = text[..open].rfind('\n').map_or(0, |n| n + 1);
        let mut end = close + 1;
        if text[end..].starts_with(';') {
            end += 1;
        }
        // Inside the quotes, so respelling the script leaves them in place.
        let quoted = args[0].start + 1..args[0].end - 1;
        return Some(Binding::Call {
            text: text[start..end].to_string(),
            script: quoted.start - start..quoted.end - start,
            function: args[1].start - start..args[1].end - start,
            end,
        });
    }
    None
}

// ── Writing edits back ────────────────────────────────────────────────────────

/// What a sync did, and what it declined to do.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SyncReport {
    /// Source changes made: value replacements plus safely moved effect start/stop blocks.
    pub changed: usize,
    /// Files whose contents actually differ afterwards.
    pub files: Vec<PathBuf>,
    /// Edits that could not be expressed as a value change, each with the reason — and notes on
    /// edits carried out somewhere other than the call, such as a const's definition.
    pub skipped: Vec<String>,
}

/// A pending value replacement: span in the file, and the text to put there.
#[derive(Debug, Clone)]
struct Replacement {
    span: Range<usize>,
    value: String,
}

/// Rewrite `text` with every replacement applied. Spans must not overlap.
fn apply(text: &str, edits: Vec<Replacement>) -> String {
    apply_mapped(text, edits).0
}

/// [`apply`], with where each offset of `text` that no edit covers lands in the result.
fn apply_mapped(text: &str, mut edits: Vec<Replacement>) -> (String, impl Fn(usize) -> usize) {
    // An empty span is an argument a project helper fixes (see `scan_macro_sites`): the value
    // lives in the helper's body, not at this call, so writing here would add an argument.
    edits.retain(|edit| {
        let phantom = edit.span.is_empty() && !edit.value.is_empty();
        if phantom {
            crate::project_helpers::refuse_fixed();
        }
        !phantom
    });
    edits.sort_by_key(|e| e.span.start);
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    // (end of an applied span in `text`, total growth up to and including it)
    let mut shifts: Vec<(usize, isize)> = Vec::new();
    let mut growth = 0isize;
    for edit in edits {
        if edit.span.start < cursor {
            continue; // overlapping — the earlier edit wins
        }
        out.push_str(&text[cursor..edit.span.start]);
        out.push_str(&edit.value);
        cursor = edit.span.end;
        growth += edit.value.len() as isize - edit.span.len() as isize;
        shifts.push((edit.span.end, growth));
    }
    out.push_str(&text[cursor..]);
    let map = move |at: usize| {
        let before = shifts.partition_point(|(end, _)| *end <= at);
        let growth = before.checked_sub(1).map_or(0, |i| shifts[i].1);
        (at as isize + growth) as usize
    };
    (out, map)
}

/// Format a float the way ACMD source spells one, and only if it actually differs.
///
/// Rewriting `1.0` to `1.0000` on every sync would churn the whole file and bury the real
/// change in the diff, so a value that round-trips to the same number is left alone.
fn float_edit(text: &str, span: &Range<usize>, new: f32) -> Option<Replacement> {
    let current = text[span.clone()].trim();
    // A const or expression, not a literal — replacing it would discard the author's intent,
    // which is exactly what this path promises not to do. A project const is instead changed at
    // its definition (see `crate::project_consts`).
    let Ok(old) = current.parse::<f32>() else {
        crate::project_consts::record_edit(current, f64::from(new));
        return None;
    };
    let replacement = format_float(new);
    // Two ways there is nothing to write, and both matter. The value being unchanged is the
    // obvious one. The second is that a written value only carries four decimals, so a float
    // finer than that — a third, a dragged slider — never compares equal to what was written
    // for it. Asking "would writing this change the text?" instead of "are these floats
    // equal?" is what lets the source editor and the panels settle against each other rather
    // than each seeing the other's value as a fresh edit, forever.
    if old == new || replacement == current {
        return None;
    }
    Some(Replacement {
        span: span.clone(),
        value: replacement,
    })
}

/// Format a number for a `ToF32` slot exactly as the emitter would, and only if it differs.
///
/// `f32::to_string` is both exact and shortest, so a wind argument written `16` stays `16` and
/// only a value that genuinely needs decimals grows them. The same two-part guard as
/// [`float_edit`]: an unchanged value writes nothing, and neither does one whose spelling the
/// author already chose.
fn to_f32_edit(text: &str, span: &Range<usize>, new: f32) -> Option<Replacement> {
    let current = text[span.clone()].trim();
    let Ok(old) = current.parse::<f32>() else {
        crate::project_consts::record_edit(current, f64::from(new));
        return None;
    };
    let replacement = new.to_string();
    if old == new || replacement == current {
        return None;
    }
    Some(Replacement {
        span: span.clone(),
        value: replacement,
    })
}

fn int_edit(text: &str, span: &Range<usize>, new: i64) -> Option<Replacement> {
    let current = text[span.clone()].trim();
    let Ok(old) = current.parse::<i64>() else {
        crate::project_consts::record_edit(current, new as f64);
        return None;
    };
    (old != new).then(|| Replacement {
        span: span.clone(),
        value: new.to_string(),
    })
}

fn bool_edit(text: &str, span: &Range<usize>, new: bool) -> Option<Replacement> {
    let current = text[span.clone()].trim();
    let old = match current {
        "false" => false,
        "true" => true,
        _ => return None,
    };
    let replacement = new.to_string();
    if old == new || replacement == current {
        return None;
    }
    Some(Replacement {
        span: span.clone(),
        value: replacement,
    })
}

/// `2.0`, `0.75`, `-1.5` — always with a decimal point, since these slots are floats.
pub(crate) fn format_float(value: f32) -> String {
    let mut text = format!("{value:.4}");
    while text.ends_with('0') && !text.ends_with(".0") {
        text.pop();
    }
    text
}

#[derive(Debug, Clone)]
struct EffectSourceBlock {
    range: Range<usize>,
    frame_arg: Range<usize>,
    frame: u32,
    execute_body: Range<usize>,
}

#[derive(Debug, Clone)]
struct EffectTimingMove {
    call_index: usize,
    new_start: u32,
    new_end: u32,
    move_start: bool,
    move_end: bool,
}

fn effect_source_is_flat(stmts: &[crate::data::EffectStmt]) -> bool {
    stmts.iter().all(|stmt| match stmt {
        crate::data::EffectStmt::Frame(_) => true,
        crate::data::EffectStmt::Excute(macros) => macros.iter().all(|macro_stmt| {
            !matches!(
                macro_stmt,
                crate::data::EffectMacro::Raw(_) | crate::data::EffectMacro::NativeCall(_)
            )
        }),
        crate::data::EffectStmt::Wait(_)
        | crate::data::EffectStmt::NativeExecuteFrame(_)
        | crate::data::EffectStmt::NativeMotionLoopSync
        | crate::data::EffectStmt::Loop { .. }
        | crate::data::EffectStmt::Cond { .. }
        | crate::data::EffectStmt::BareNativeCall(_)
        | crate::data::EffectStmt::BareEffect(_)
        | crate::data::EffectStmt::Raw(_) => false,
    })
}

fn all_effect_source_sites(text: &str) -> Vec<MacroSite> {
    let mut sites = scan_macro_sites(text, 0..text.len());
    sites.extend(scan_named_sites(
        text,
        "visionary_last_effect_set_scale_w",
        0..text.len(),
    ));
    sites.extend(native_effect_control_sites(text));
    sites.sort_by_key(|site| site.span.start);
    sites
}

fn native_effect_control_sites(text: &str) -> Vec<MacroSite> {
    [
        "EffectModule::set_disable_render_offset_last",
        "EffectModule::enable_sync_init_pos_last",
    ]
    .into_iter()
    .flat_map(|name| scan_named_sites(text, name, 0..text.len()))
    .collect()
}

fn is_native_effect_control_name(name: &str) -> bool {
    matches!(
        name,
        "EffectModule::set_disable_render_offset_last" | "EffectModule::enable_sync_init_pos_last"
    )
}

/// Match only the physical, direct-in-execute form the effect parser types. Keeping this
/// predicate in lockstep with that parser is essential: every typed control consumes one call
/// ordinal, while malformed/two-argument source remains Raw and consumes none.
fn is_typed_native_effect_control(text: &str, site: &MacroSite) -> bool {
    if !is_native_effect_control_name(&site.name)
        || site.args.len() != 1
        || site.arg(text, 0).map(str::trim) != Some("agent.module_accessor")
    {
        return false;
    }
    let Some(line) = line_range_at(text, site.span.start).and_then(|range| text.get(range)) else {
        return false;
    };
    let expected = format!("{}(agent.module_accessor);", site.name);
    if line.trim() != expected.as_str() {
        return false;
    }

    let Some((open, _close)) = enclosing_effect_execute(text, site.span.start) else {
        return false;
    };
    // Reuse the parser's exact physical body boundary and root-statement mask: counting a
    // control the importer retained as Raw would shift every later source-edit ordinal.
    let header_line = text[..open].bytes().filter(|byte| *byte == b'\n').count();
    let source_line = text[..site.span.start]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count();
    let lines = text.lines().collect::<Vec<_>>();
    let (body_end, _) = crate::acmd::find_block_end(&lines, header_line);
    if source_line <= header_line || source_line >= body_end {
        return false;
    }
    crate::acmd::direct_native_statement_lines(&lines[header_line + 1..body_end])
        .contains(&(source_line - header_line - 1))
}

fn line_range_at(text: &str, offset: usize) -> Option<Range<usize>> {
    source_line_ranges(text)
        .into_iter()
        .find(|range| range.start <= offset && offset < range.end)
}

fn enclosing_effect_execute(text: &str, site_start: usize) -> Option<(usize, usize)> {
    // Use the same exact physical header the effect parser recognizes, but discover headers
    // with the comment/string-aware source scanner. A raw `rfind` can select `is_excute` text
    // inside a comment or literal and then silently drop a real native control from ordinals.
    for header in scan_macro_sites(text, 0..site_start)
        .into_iter()
        .filter(|site| site.name == "is_excute" && site.args.len() == 1)
    {
        let Some(line) = line_range_at(text, header.span.start) else {
            continue;
        };
        // An `if` on this function's own agent, under whatever name it gives it, written over
        // several lines or as one (`if macros::is_excute(agent) { EFFECT(…); }`).
        // The same headers the effect parser reads as an Execute block.
        if !crate::acmd::is_plain_excute_header(text.get(line.clone())?) {
            continue;
        }
        let open = header.span.end + text[header.span.end..line.end].find('{')?;
        if text.as_bytes().get(open) != Some(&b'{') {
            continue;
        }
        let Some(close) = matching_brace(text, open) else {
            continue;
        };
        if open < site_start && site_start <= close {
            return Some((open, close));
        }
    }
    None
}

fn source_frame_block(text: &str, site: &MacroSite) -> Option<EffectSourceBlock> {
    let frames = scan_named_sites(text, "frame", 0..text.len());
    let frame_site = frames
        .iter()
        .rev()
        .find(|frame| frame.span.end <= site.span.start)?;
    let frame_line = line_range_at(text, frame_site.span.start)?;
    let frame_text = text.get(frame_line.clone())?.trim();
    if !frame_text.starts_with("frame(") {
        return None;
    }
    let frame_value = frame_site
        .args
        .get(1)
        .and_then(|span| text.get(span.clone()))?
        .trim()
        .parse::<f32>()
        .ok()?;
    let (open, close) = enclosing_effect_execute(text, site.span.start)?;
    let execute_line = line_range_at(text, open)?;
    // A timing move carries the frame statement and its execute block together. Comments and
    // whitespace between them are safe; executable text there would be an unowned side effect.
    let between = &text[frame_line.end..execute_line.start];
    if between.lines().any(|line| {
        let line = line.trim();
        !line.is_empty() && !line.starts_with("//") && !line.starts_with("/*")
    }) {
        return None;
    }
    let block_end = line_range_at(text, close)
        .map(|line| line.end)
        .unwrap_or(close + 1);
    let block_end = block_end.min(text.len());
    Some(EffectSourceBlock {
        range: frame_line.start..block_end,
        frame_arg: frame_site.args.get(1)?.clone(),
        frame: crate::data::script_frame(frame_value),
        execute_body: open + 1..close,
    })
}

fn site_inside(range: &Range<usize>, site: &MacroSite) -> bool {
    range.start <= site.span.start && site.span.end <= range.end
}

fn effect_block_has_only(
    text: &str,
    block: &EffectSourceBlock,
    target: &MacroSite,
    all_sites: &[MacroSite],
    allow_modifiers: bool,
) -> bool {
    let contained: Vec<&MacroSite> = all_sites
        .iter()
        .filter(|site| site_inside(&block.execute_body, site))
        .collect();
    if contained
        .iter()
        .filter(|site| site.span == target.span)
        .count()
        != 1
    {
        return false;
    }
    if contained.iter().any(|site| {
        if site.span == target.span {
            return false;
        }
        allow_modifiers && MODIFIER_COMMANDS.contains(&site.name.as_str())
    }) {
        // All non-target sites must be recognized direct modifiers. The expression is written
        // this way so a future command added to the scanner cannot become movable by accident.
        if contained.iter().any(|site| {
            site.span != target.span && !MODIFIER_COMMANDS.contains(&site.name.as_str())
        }) {
            return false;
        }
    } else if contained.iter().any(|site| site.span != target.span) {
        return false;
    }

    let mut body = text[block.execute_body.clone()].to_string();
    for site in contained {
        if site.span == target.span
            || (allow_modifiers && MODIFIER_COMMANDS.contains(&site.name.as_str()))
        {
            if allow_modifiers && site.span != target.span && site.span.start < target.span.end {
                // A recognized modifier before the call is not attached to this call by the
                // parser's last-effect rule. Moving it with the call would silently change
                // which effect it targets, so treat the block as ambiguous.
                return false;
            }
            let start = site.span.start - block.execute_body.start;
            let mut end = site.span.end - block.execute_body.start;
            if text
                .get(site.span.end..)
                .is_some_and(|tail| tail.starts_with(';'))
            {
                end += 1;
            }
            if start < body.len() && end <= body.len() && start < end {
                body.replace_range(start..end, &" ".repeat(end - start));
            }
        }
    }
    // Keep comments attached to the moved block, but reject any executable residue the editor
    // cannot identify as belonging to this call.
    body.lines().all(|line| {
        let line = line.trim();
        line.is_empty()
            || line.starts_with("//")
            || line.starts_with("/*")
            || line.starts_with('*')
            || line.ends_with("*/")
    })
}

fn source_frame_blocks(text: &str) -> Vec<EffectSourceBlock> {
    let frames = scan_named_sites(text, "frame", 0..text.len());
    frames
        .iter()
        .enumerate()
        .filter_map(|(index, site)| {
            let line = line_range_at(text, site.span.start)?;
            let value = site
                .args
                .get(1)
                .and_then(|span| text.get(span.clone()))?
                .trim()
                .parse::<f32>()
                .ok()?;
            // A frame block ends at the next frame statement. Never search past that
            // boundary: otherwise a frame with no execute block can accidentally claim the
            // next frame's execute body and become a false collision/move anchor.
            let limit = frames
                .get(index + 1)
                .map(|next| next.span.start)
                .unwrap_or(text.len());
            let execute = text[site.span.end..limit]
                .match_indices("is_excute(")
                .map(|(offset, _)| site.span.end + offset)
                .find(|at| {
                    line_range_at(text, *at).is_some_and(|line| opens_is_excute(&text[line]))
                })?;
            let execute_line = line_range_at(text, execute)?;
            if execute_line.start < line.end {
                return None;
            }
            let open = text[execute..execute_line.end]
                .find('{')
                .map(|offset| execute + offset)?;
            let close = matching_brace(text, open)?;
            if close >= limit {
                return None;
            }
            Some(EffectSourceBlock {
                range: line.start..line_range_at(text, close).map_or(close + 1, |r| r.end),
                frame_arg: site.args.get(1)?.clone(),
                frame: crate::data::script_frame(value),
                execute_body: open + 1..close,
            })
        })
        .collect()
}

fn source_hash_name(text: &str, span: Option<&Range<usize>>) -> Option<String> {
    let value = text.get(span?.clone())?.trim();
    let value = value.strip_prefix("Hash40::new(\"")?.strip_suffix("\")")?;
    Some(value.to_ascii_lowercase())
}

fn effect_stop_block(
    text: &str,
    call_index: usize,
    call: &crate::data::EffectCall,
    call_site: &MacroSite,
    call_sites: &[Option<MacroSite>],
    calls: &[crate::data::EffectCall],
    all_sites: &[MacroSite],
) -> Option<(MacroSite, EffectSourceBlock)> {
    let is_trail = call.trail_command.is_some()
        || call.spawn_func == "AFTER_IMAGE_ON"
        || call.raw_line.is_some();
    let stop_name = if is_trail {
        "AFTER_IMAGE_OFF"
    } else {
        "EFFECT_OFF_KIND"
    };
    let candidates: Vec<(MacroSite, EffectSourceBlock)> = all_sites
        .iter()
        .filter(|site| site.name == stop_name && site.span.start > call_site.span.end)
        .filter_map(|site| {
            if !is_trail
                && source_hash_name(text, site.args.get(1)).as_deref()
                    != Some(call.effect_name.to_ascii_lowercase().as_str())
            {
                return None;
            }
            let block = source_frame_block(text, site)?;
            (block.frame == call.active_end).then_some((site.clone(), block))
        })
        .filter(|(site, block)| effect_block_has_only(text, block, site, all_sites, false))
        .collect();
    if candidates.len() != 1 {
        return None;
    }
    let (stop_site, stop_block) = &candidates[0];
    let paired = calls
        .iter()
        .enumerate()
        .filter_map(|(index, other)| {
            let site = call_sites.get(index)?.as_ref()?;
            Some((other, site))
        })
        .filter(|(other, site)| {
            site.span.start < stop_site.span.start
                && other.active_end == call.active_end
                && if is_trail {
                    other.trail_command.is_some()
                        || other.spawn_func == "AFTER_IMAGE_ON"
                        || other.raw_line.is_some()
                } else {
                    other.follows_bone && other.effect_name.eq_ignore_ascii_case(&call.effect_name)
                }
        })
        .count();
    if paired != 1 || calls.get(call_index).is_none() {
        return None;
    }
    Some((stop_site.clone(), stop_block.clone()))
}

fn plan_effect_timing_moves(
    text: &str,
    label: &str,
    script: &crate::data::EffectScript,
    pristine: &[crate::data::EffectCall],
    edited: &[crate::data::EffectCall],
    ordinals: &[usize],
    sites: &[MacroSite],
) -> (Vec<EffectTimingMove>, Vec<String>) {
    let mut moves = Vec::new();
    let mut skipped = Vec::new();
    let timing_indices: Vec<usize> = pristine
        .iter()
        .zip(edited)
        .enumerate()
        .filter(|(_, (before, after))| {
            before.active_start != after.active_start
                || (before.follows_bone && before.active_end != after.active_end)
        })
        .map(|(index, _)| index)
        .collect();
    if timing_indices.is_empty() {
        return (moves, skipped);
    }
    if !effect_source_is_flat(&script.stmts) {
        return (
            moves,
            timing_indices
                .into_iter()
                .map(|index| {
                    format!(
                        "{label}: effect #{} has timing changes inside a branch, loop, wait, or opaque source statement",
                        index + 1
                    )
                })
                .collect(),
        );
    }
    let all_sites = all_effect_source_sites(text);
    let frame_blocks = source_frame_blocks(text);
    let call_blocks: Vec<Option<EffectSourceBlock>> = ordinals
        .iter()
        .map(|ordinal| {
            sites
                .get(*ordinal)
                .and_then(|site| source_frame_block(text, site))
        })
        .collect();
    let call_sites: Vec<Option<MacroSite>> = ordinals
        .iter()
        .map(|ordinal| sites.get(*ordinal).cloned())
        .collect();
    let mut proposed_frames: HashMap<u32, usize> = HashMap::new();

    for index in timing_indices {
        let before = &pristine[index];
        let after = &edited[index];
        let Some(site) = sites.get(*ordinals.get(index).unwrap_or(&usize::MAX)) else {
            skipped.push(format!(
                "{label}: effect #{} has no uniquely identifiable source call",
                index + 1
            ));
            continue;
        };
        let Some(start_block) = call_blocks.get(index).and_then(Clone::clone) else {
            skipped.push(format!(
                "{label}: `{}` effect #{} is not an isolated top-level frame block",
                site.name,
                index + 1
            ));
            continue;
        };
        if !effect_block_has_only(text, &start_block, site, &all_sites, true) {
            skipped.push(format!(
                "{label}: `{}` effect #{} shares its frame block with unrelated source lines",
                site.name,
                index + 1
            ));
            continue;
        }
        let move_start = before.active_start != after.active_start;
        let move_end = before.follows_bone && before.active_end != after.active_end;
        if after.active_end < after.active_start {
            skipped.push(format!(
                "{label}: `{}` effect #{} would end before it starts",
                site.name,
                index + 1
            ));
            continue;
        }
        let stop_block = if move_end {
            if !before.follows_bone
                || before.active_end == crate::data::OPEN_ENDED_EFFECT_FRAME
                || after.active_end == crate::data::OPEN_ENDED_EFFECT_FRAME
            {
                skipped.push(format!(
                    "{label}: `{}` effect #{} would require creating or removing a stop command",
                    site.name,
                    index + 1
                ));
                continue;
            }
            effect_stop_block(text, index, before, site, &call_sites, pristine, &all_sites)
        } else {
            None
        };
        if move_end && stop_block.is_none() {
            skipped.push(format!(
                "{label}: `{}` effect #{} has no unique paired stop block",
                site.name,
                index + 1
            ));
            continue;
        }
        if move_start && start_block.frame == after.active_start {
            // A stale editor record can report a timing difference after the source already
            // contains the requested frame. Treat that as settled instead of moving text.
            continue;
        }
        if move_start {
            if frame_blocks
                .iter()
                .any(|block| block.frame == after.active_start && block.range != start_block.range)
            {
                skipped.push(format!(
                    "{label}: `{}` effect #{} targets an occupied frame block",
                    site.name,
                    index + 1
                ));
                continue;
            }
            if proposed_frames.insert(after.active_start, index).is_some() {
                skipped.push(format!(
                    "{label}: multiple effect calls target frame {} and cannot be ordered safely",
                    after.active_start
                ));
                continue;
            }
        }
        if let Some((_, block)) = &stop_block {
            if frame_blocks.iter().any(|candidate| {
                candidate.frame == after.active_end && candidate.range != block.range
            }) {
                skipped.push(format!(
                    "{label}: `{}` effect #{} targets an occupied stop frame block",
                    site.name,
                    index + 1
                ));
                continue;
            }
            if proposed_frames.insert(after.active_end, index).is_some() {
                skipped.push(format!(
                    "{label}: multiple effect stops target frame {} and cannot be ordered safely",
                    after.active_end
                ));
                continue;
            }
        }
        moves.push(EffectTimingMove {
            call_index: index,
            new_start: after.active_start,
            new_end: after.active_end,
            move_start,
            move_end,
        });
    }
    (moves, skipped)
}

fn apply_effect_timing_moves(
    text: &str,
    moves: &[EffectTimingMove],
    calls: &[crate::data::EffectCall],
    ordinals: &[usize],
    sites: &[MacroSite],
    value_edits: &[Replacement],
) -> Option<String> {
    if moves.is_empty() {
        return Some(apply(text, value_edits.to_vec()));
    }
    let all_sites = all_effect_source_sites(text);
    let call_sites: Vec<Option<MacroSite>> = ordinals
        .iter()
        .map(|ordinal| sites.get(*ordinal).cloned())
        .collect();
    let mut chunks: Vec<(Range<usize>, String, u32)> = Vec::new();
    for move_plan in moves {
        let site = call_sites.get(move_plan.call_index)?.as_ref()?;
        let start_block = source_frame_block(text, site)?;
        if !effect_block_has_only(text, &start_block, site, &all_sites, true) {
            return None;
        }
        let mut start_edits: Vec<Replacement> = value_edits
            .iter()
            .filter(|edit| {
                start_block.range.start <= edit.span.start && edit.span.end <= start_block.range.end
            })
            .map(|edit| Replacement {
                span: edit.span.start - start_block.range.start
                    ..edit.span.end - start_block.range.start,
                value: edit.value.clone(),
            })
            .collect();
        start_edits.push(Replacement {
            span: start_block.frame_arg.start - start_block.range.start
                ..start_block.frame_arg.end - start_block.range.start,
            value: format_float(move_plan.new_start as f32),
        });
        let start_chunk = apply(text[start_block.range.clone()].as_ref(), start_edits);
        chunks.push((start_block.range, start_chunk, move_plan.new_start));
        if move_plan.move_end {
            let stop = effect_stop_block(
                text,
                move_plan.call_index,
                calls.get(move_plan.call_index)?,
                site,
                &call_sites,
                calls,
                &all_sites,
            )?
            .1;
            let mut stop_edits: Vec<Replacement> = value_edits
                .iter()
                .filter(|edit| {
                    stop.range.start <= edit.span.start && edit.span.end <= stop.range.end
                })
                .map(|edit| Replacement {
                    span: edit.span.start - stop.range.start..edit.span.end - stop.range.start,
                    value: edit.value.clone(),
                })
                .collect();
            stop_edits.push(Replacement {
                span: stop.frame_arg.start - stop.range.start
                    ..stop.frame_arg.end - stop.range.start,
                value: format_float(move_plan.new_end as f32),
            });
            let stop_chunk = apply(text[stop.range.clone()].as_ref(), stop_edits);
            chunks.push((stop.range, stop_chunk, move_plan.new_end));
        }
    }
    if chunks.iter().enumerate().any(|(i, (left, _, _))| {
        chunks
            .iter()
            .skip(i + 1)
            .any(|(right, _, _)| left.start < right.end && right.start < left.end)
    }) {
        return None;
    }

    let frame_blocks = source_frame_blocks(text);
    let moved_ranges: Vec<Range<usize>> =
        chunks.iter().map(|(range, _, _)| range.clone()).collect();
    let retained_value_edits: Vec<Replacement> = value_edits
        .iter()
        .filter(|edit| {
            !moved_ranges
                .iter()
                .any(|range| range.start <= edit.span.start && edit.span.end <= range.end)
        })
        .cloned()
        .collect();
    let mut insertions: HashMap<usize, Vec<(u32, String)>> = HashMap::new();
    for (range, chunk, frame) in &chunks {
        let anchor = frame_blocks
            .iter()
            .filter(|block| {
                !moved_ranges.iter().any(|moved| moved == &block.range) && block.frame > *frame
            })
            .min_by_key(|block| block.frame)
            .map(|block| block.range.start)
            .or_else(|| {
                frame_blocks
                    .iter()
                    .filter(|block| !moved_ranges.iter().any(|moved| moved == &block.range))
                    .map(|block| block.range.end)
                    .max()
            })
            // Moving the only block in a function has no surviving frame block to anchor
            // against. Insert it immediately before the function's closing brace rather than
            // appending it after the function, which would produce invalid Rust.
            .or_else(|| text.rfind('}'))
            .unwrap_or(text.len());
        insertions
            .entry(anchor)
            .or_default()
            .push((*frame, chunk.clone()));
        let _ = range;
    }
    for chunks_at_anchor in insertions.values_mut() {
        chunks_at_anchor.sort_by_key(|(frame, _)| *frame);
    }

    let mut anchors: Vec<usize> = insertions.keys().copied().collect();
    anchors.sort_unstable();
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for anchor in anchors {
        if anchor < cursor {
            continue;
        }
        append_effect_text_without_ranges(
            &mut out,
            text,
            cursor..anchor,
            &moved_ranges,
            &retained_value_edits,
        );
        for (_, chunk) in &insertions[&anchor] {
            out.push_str(chunk);
        }
        cursor = anchor;
    }
    append_effect_text_without_ranges(
        &mut out,
        text,
        cursor..text.len(),
        &moved_ranges,
        &retained_value_edits,
    );
    Some(out)
}

fn append_effect_text_without_ranges(
    out: &mut String,
    text: &str,
    range: Range<usize>,
    removed: &[Range<usize>],
    edits: &[Replacement],
) {
    let mut cursor = range.start;
    let mut removed_ranges: Vec<_> = removed
        .iter()
        .filter(|span| span.start < range.end && span.end > range.start)
        .collect();
    removed_ranges.sort_by_key(|span| span.start);
    for span in removed_ranges {
        if span.start > cursor {
            append_effect_text_with_edits(out, text, cursor..span.start.min(range.end), edits);
        }
        cursor = cursor.max(span.end);
    }
    if cursor < range.end {
        append_effect_text_with_edits(out, text, cursor..range.end, edits);
    }
}

fn append_effect_text_with_edits(
    out: &mut String,
    text: &str,
    range: Range<usize>,
    edits: &[Replacement],
) {
    let mut applicable: Vec<&Replacement> = edits
        .iter()
        .filter(|edit| range.start <= edit.span.start && edit.span.end <= range.end)
        .collect();
    applicable.sort_by_key(|edit| edit.span.start);
    let mut cursor = range.start;
    for edit in applicable {
        if edit.span.start < cursor {
            continue;
        }
        out.push_str(&text[cursor..edit.span.start]);
        out.push_str(&edit.value);
        cursor = edit.span.end;
    }
    out.push_str(&text[cursor..range.end]);
}

/// Compare effect calls as an unordered semantic set after a source rewrite.
///
/// Moving a frame block changes source order, so a positional comparison would reject a valid
/// retime as soon as it crosses another call. The serialized call is a convenient complete
/// fingerprint here: it includes the typed arguments, modifiers, guards, and preserved raw
/// identity without introducing a second list of fields that can drift from `EffectCall`.
fn effect_call_set_signature(calls: &[crate::data::EffectCall]) -> Vec<String> {
    let mut signatures: Vec<String> = calls
        .iter()
        .cloned()
        .map(crate::data::EffectCall::normalized_timing)
        .map(canonicalize_effect_call_for_source)
        .map(|call| serde_json::to_string(&call).unwrap_or_else(|_| format!("{call:?}")))
        .collect();
    signatures.sort();
    signatures
}

/// Whether every signature in `wanted` is present in `actual`, with duplicates counted.
///
/// A source rewrite may also carry a separately reported rename or removal that this path
/// deliberately refuses. Added calls still need an independent read-back check in that case,
/// so comparing the whole edited list would reject a valid addition because of the refused
/// structural edit beside it.
fn effect_signatures_contain(actual: &[String], wanted: &[String]) -> bool {
    let mut remaining = actual.to_vec();
    for signature in wanted {
        let Some(position) = remaining
            .iter()
            .position(|candidate| candidate == signature)
        else {
            return false;
        };
        remaining.remove(position);
    }
    true
}

/// Whether two parsed hitboxes identify the same source call.
///
/// Values are intentionally absent. The caller is comparing the source it is about to edit
/// with the editor's pristine snapshot, so a value difference is the work to perform. The
/// frame, family, and family-specific identity are the anchors that make a positional source
/// site safe to use when an id is reused later in the move.
fn hitbox_source_identity(a: &crate::data::Hitbox, b: &crate::data::Hitbox) -> bool {
    if a.category != b.category
        || a.func != b.func
        || a.id != b.id
        || a.part != b.part
        || a.active_start != b.active_start
    {
        return false;
    }
    match a.category {
        crate::data::CAT_ABS => {
            a.abs.as_ref().map(|value| value.kind.as_str())
                == b.abs.as_ref().map(|value| value.kind.as_str())
        }
        2 => {
            a.wind.as_ref().map(|value| value.command.as_str())
                == b.wind.as_ref().map(|value| value.command.as_str())
        }
        _ => true,
    }
}

/// Whether a scanned macro site has the identity the parser assigned to one hitbox.
///
/// This is the guard against a coincidental count match. If an unsupported or malformed macro
/// disappears from the parser while a different collision family takes its place, zipping the
/// two lists must fall back to the older unique-match path instead of retuning the neighbour.
/// A numeric id/part argument of `site`: a literal, or a project const the parser resolved
/// (`THROWB_STAB_HITBOX_ID as u64`).
fn site_u32(text: &str, site: &MacroSite, slot: usize) -> Option<u32> {
    let value = site.arg(text, slot)?.trim();
    value.parse::<u32>().ok().or_else(|| {
        crate::project_consts::resolve_in_scope(value)
            .filter(|v| v.fract() == 0.0 && *v >= 0.0 && *v <= f64::from(u32::MAX))
            .map(|v| v as u32)
    })
}

fn hitbox_site_matches(text: &str, site: &MacroSite, hitbox: &crate::data::Hitbox) -> bool {
    let number = |slot: usize| site_u32(text, site, slot);
    let hash = |slot: usize| {
        site.arg(text, slot)
            .map(|value| value.trim().trim_start_matches('*'))
    };
    match hitbox.category {
        crate::data::CAT_ABS => {
            site.name == "ATTACK_ABS"
                && number(2) == Some(hitbox.id)
                && hash(1) == hitbox.abs.as_ref().map(|value| value.kind.as_str())
        }
        crate::data::CAT_ATTACK_FP => {
            site.name == "ATTACK_FP"
                && number(1) == Some(hitbox.id)
                && number(2) == Some(hitbox.part)
        }
        1 => site.name == "CATCH" && number(1) == Some(hitbox.id),
        crate::data::CAT_SEARCH => {
            site.name == "SEARCH" && number(1) == Some(hitbox.id) && number(2) == Some(hitbox.part)
        }
        2 => {
            site.name
                == hitbox
                    .wind
                    .as_ref()
                    .map(|value| value.command.as_str())
                    .unwrap_or_default()
                && number(1) == Some(hitbox.id)
        }
        _ => {
            crate::acmd::ATTACK_FUNCS.contains(&site.name.as_str())
                && site.name == hitbox.func
                && number(1) == Some(hitbox.id)
                && number(2) == Some(hitbox.part)
        }
    }
}

/// Map parsed hitboxes to source sites when the source is a one-to-one collision list.
///
/// A loop intentionally produces more parsed hitboxes than source sites, and a malformed call
/// can make the parser/source counts disagree. Those cases return no positional anchors and
/// retain the existing unique-match refusal. A flat script with the same id/part reused at two
/// frames does have a safe anchor: its parsed order and frame identity select the corresponding
/// source call without guessing from id alone.
fn positional_hitbox_sites<'a>(
    text: &str,
    sites: &'a [MacroSite],
    pristine: &[crate::data::Hitbox],
) -> Vec<Option<&'a MacroSite>> {
    let mut mapped = vec![None; pristine.len()];
    let parsed = crate::acmd::parse_acmd_script(text).to_hitboxes();
    let collision_sites: Vec<&MacroSite> = sites
        .iter()
        .filter(|site| {
            crate::acmd::ATTACK_FUNCS.contains(&site.name.as_str())
                || site.name == "ATTACK_FP"
                || site.name == "CATCH"
                || site.name == "ATTACK_ABS"
                || site.name == "SEARCH"
                || crate::data::is_wind_command(&site.name)
        })
        .collect();
    if parsed.len() != collision_sites.len() || parsed.len() != pristine.len() {
        return mapped;
    }
    // Validate the complete correspondence before exposing any anchor. A single source change
    // can otherwise leave an early pair coincidentally aligned while a later pair shifted; using
    // only the coincidental entries would still let one edit reach the wrong call.
    let valid =
        parsed
            .iter()
            .zip(pristine)
            .zip(&collision_sites)
            .all(|((source, before), site)| {
                hitbox_source_identity(source, before) && hitbox_site_matches(text, site, source)
            });
    if !valid {
        return mapped;
    }
    for (position, site) in collision_sites.into_iter().enumerate() {
        mapped[position] = Some(site);
    }
    mapped
}

fn source_compare_float(value: f32) -> f32 {
    if value.is_finite() {
        format!("{value:.4}").parse().unwrap_or(value)
    } else {
        value
    }
}

fn canonicalize_effect_call_for_source(
    mut call: crate::data::EffectCall,
) -> crate::data::EffectCall {
    // `flip_axis` is the typed view of one token that is also retained in `extra_args` for
    // lossless export. Callers may edit the typed field directly, so make the two views agree
    // before comparing a source rewrite with the edited model.
    if let (Some(axis), Some(tail_index)) = (
        call.flip_axis.clone(),
        crate::acmd::effect_flip_axis_tail_index(&call.spawn_func),
    ) {
        if let Some(extra_args) = call.extra_args.as_mut() {
            if let Some(slot) = extra_args.get_mut(tail_index) {
                *slot = axis;
            }
        }
    }
    // A stop the editor never set is written with the defaults (see `acmd::emit_spawn_stop`),
    // which the parser then reads back as set. Compare both as the line the source holds; a
    // call with no stop has neither.
    let trail = call.trail_command.is_some() || call.spawn_func == "AFTER_IMAGE_ON" || call.raw_line.is_some();
    let stopped = call.active_end != crate::data::OPEN_ENDED_EFFECT_FRAME;
    if stopped && trail {
        call.trail_off = Some(call.trail_off.unwrap_or(crate::data::TRAIL_OFF_DEFAULT));
    } else if stopped && call.follows_bone {
        let (fade, detach) = crate::data::EFFECT_OFF_KIND_DEFAULT;
        call.off_fade = Some(call.off_fade.unwrap_or(fade));
        call.off_detach = Some(call.off_detach.unwrap_or(detach));
    }
    if !stopped {
        call.off_fade = None;
        call.off_detach = None;
    }
    call.offset = call.offset.map(source_compare_float);
    call.rotation = call.rotation.map(source_compare_float);
    call.scale = source_compare_float(call.scale);
    call.rate = call.rate.map(source_compare_float);
    call.camera_offset = call.camera_offset.map(source_compare_float);
    call.tint = call.tint.map(|values| values.map(source_compare_float));
    call.particle_tint = call
        .particle_tint
        .map(|values| values.map(source_compare_float));
    call.alpha = call.alpha.map(source_compare_float);
    call.scale_w = call
        .scale_w
        .map(|values| values.into_iter().map(source_compare_float).collect());
    if let Some(color) = call.color.as_mut() {
        color.transition = color.transition.map(source_compare_float);
        color.rgba = color.rgba.map(|values| values.map(source_compare_float));
    }
    call
}

/// Rewrite one `effect_*` function's source with the editor's edited transform values.
///
/// Pure: `text` is the function on its own, and the result is that function rewritten. This
/// is the shared core of both write-back routes — the explicit sync, which splices the result
/// back into the project file, and the source editor window, which is already holding exactly
/// this text and simply swaps it.
///
/// Position, rotation, modifier values, and bounded timing moves are written when the source
/// structure identifies exactly one safe block. Additions, removals, renames, ambiguous blocks,
/// and stop creation/removal remain reported rather than approximated.
pub fn rewrite_effect_calls(
    text: &str,
    label: &str,
    pristine: &[crate::data::EffectCall],
    edited: &[crate::data::EffectCall],
) -> Result<(String, SyncReport)> {
    let script = crate::acmd::parse_effect_script(text);
    let ordinals = script.call_macro_ordinals();
    let (sites, modifier_sites) = spawn_and_modifier_sites(text);
    let pristine: Vec<crate::data::EffectCall> = pristine
        .iter()
        .cloned()
        .map(crate::data::EffectCall::normalized_timing)
        .collect();
    let edited: Vec<crate::data::EffectCall> = edited
        .iter()
        .cloned()
        .map(crate::data::EffectCall::normalized_timing)
        .collect();

    let mut report = SyncReport::default();
    if ordinals.len() != pristine.len() {
        bail!(
            "{label}: the editor holds {} spawns but the source parses to {} — reload the \
             move from source before syncing",
            pristine.len(),
            ordinals.len()
        );
    }
    if edited.len() < pristine.len() {
        report.skipped.push(format!(
            "{label}: {} effect spawn(s) were removed — source syncing does not delete existing calls",
            pristine.len() - edited.len()
        ));
        // The remaining list no longer has positional identity: after removing the first call,
        // edited[0] describes the old second call. Applying value edits before a removal map is
        // proven would retune the wrong authored spawn, so leave the complete source untouched.
        return Ok((text.to_string(), report));
    }

    let (mut timing_moves, mut timing_skipped) =
        plan_effect_timing_moves(text, label, &script, &pristine, &edited, &ordinals, &sites);
    // The block mover above needs a flat `frame` script with each spawn alone in its block.
    // When it refuses, every retime goes through the line mover instead, which follows `wait`s
    // and moves just the spawn's own lines; its refusal keeps the block mover's reasons.
    let line_retimes: Vec<usize> = if timing_skipped.is_empty() || edited.len() != pristine.len() {
        Vec::new()
    } else {
        pristine
            .iter()
            .zip(&edited)
            .enumerate()
            .filter(|(_, (before, after))| {
                before.active_start != after.active_start || before.active_end != after.active_end
            })
            .filter(|(_, (before, after))| structural_identity_matches(before, after))
            .map(|(index, _)| index)
            .collect()
    };
    if line_retimes.is_empty() {
        report.skipped.append(&mut timing_skipped);
    } else {
        timing_moves.clear();
    }
    // Timing moves are only valid for a one-to-one list whose call identity stays the same.
    // Otherwise a rename/disable/add/remove could still carry a previously approved block move
    // through the value-writing pass and leave a partial structural edit in the source.
    if edited.len() != pristine.len() {
        timing_moves.clear();
    } else {
        timing_moves.retain(|timing| {
            pristine
                .get(timing.call_index)
                .zip(edited.get(timing.call_index))
                .is_some_and(|(before, after)| structural_identity_matches(before, after))
        });
    }

    // A macro inside a `for` produces one call per iteration off ONE line of text. Only sync
    // it when every iteration agrees, so an edit to a single iteration is refused instead of
    // silently rewriting all of them.
    let mut per_site: HashMap<usize, Vec<usize>> = HashMap::new();
    for (call_index, ordinal) in ordinals.iter().enumerate() {
        per_site.entry(*ordinal).or_default().push(call_index);
    }

    let mut edits: Vec<Replacement> = Vec::new();
    // Added spawns (indices past the pristine list) are inserted as new frame
    // blocks below, after the value/timing passes. They are not a refusal.

    // The stop command's own arguments live on a different line from the spawn, so they are
    // resolved against the whole source rather than the spawn's site.
    let all_sites = all_effect_source_sites(text);
    let call_sites: Vec<Option<MacroSite>> = ordinals
        .iter()
        .map(|ordinal| sites.get(*ordinal).cloned())
        .collect();

    for (ordinal, call_indices) in per_site {
        let Some(macro_site) = sites.get(ordinal) else {
            report.skipped.push(format!(
                "{label}: spawn #{ordinal} has no matching line in the source"
            ));
            continue;
        };
        // Any difference at all, not just a transform one. Selecting on the transform alone
        // meant an edit that changed only the graphic, the joint, the frame, or the disabled
        // flag was neither written nor reported: the user renamed an effect, pressed Save,
        // and the source silently kept the old name with the report claiming success.
        let differs: Vec<usize> = call_indices
            .iter()
            .copied()
            .filter(|i| edited.get(*i).is_some_and(|e| &pristine[*i] != e))
            .collect();
        if differs.is_empty() {
            continue;
        }
        if differs
            .iter()
            .any(|i| !structural_identity_matches(&pristine[*i], &edited[*i]))
        {
            report.skipped.push(format!(
                "{label}: `{}` changed graphic, joint, spawn command, timing, or enablement — source syncing \
                 will not make this structural change; it writes values and unambiguous timing blocks",
                macro_site.name
            ));
            continue;
        }
        let timing_changed = differs.iter().any(|i| {
            pristine[*i].active_start != edited[*i].active_start
                || (pristine[*i].follows_bone && pristine[*i].active_end != edited[*i].active_end)
        });
        let timing_written = differs
            .iter()
            .all(|i| timing_moves.iter().any(|timing| timing.call_index == *i));
        if timing_changed && !timing_written {
            // The planning pass already supplied the precise refusal reason. Continue through
            // the value writers so a safe transform edit on the same call is not lost merely
            // because its source timing block was ambiguous.
        }
        // A trail carries textures and per-frame trail parameters, and no transform at all —
        // its joints are arguments 4 onward. Applying the spawn layout here overwrote the
        // user's own trail settings with position values and reported it as a clean success.
        if is_trail_macro(&macro_site.name) {
            if !timing_written {
                report.skipped.push(format!(
                    "{label}: `{}` has no position, rotation, or scale arguments to write — a \
                     trail is placed by the joints it names, not by a transform",
                    macro_site.name
                ));
            }
            continue;
        }
        let target = &edited[differs[0]];
        if differs.len() != call_indices.len()
            || differs
                .iter()
                .any(|i| !transform_matches(&edited[*i], target))
        {
            report.skipped.push(format!(
                "{label}: `{}` on one loop iteration differs from the others — a loop body is \
                 a single line of source and cannot hold per-iteration values",
                macro_site.name
            ));
            continue;
        }
        if let Some(control) = &target.control {
            let Some(was) = pristine[differs[0]].control.as_ref() else {
                report.skipped.push(format!(
                    "{label}: `{}` changed into a control command — source syncing does not add or replace calls",
                    macro_site.name
                ));
                continue;
            };
            control_edits(
                text,
                label,
                macro_site,
                was,
                control,
                &mut edits,
                &mut report,
            );
            continue;
        }
        // A colour command has no transform to write and no rate line beneath it — its
        // arguments are a length and four components, and nothing else.
        if let Some(color) = &target.color {
            edits.extend(color_edits(text, macro_site, color));
            continue;
        }
        let was = &pristine[differs[0]];
        if was.flip_axis != target.flip_axis {
            let axis_slot = crate::acmd::effect_flip_axis_tail_index(&target.spawn_func)
                .map(|tail| 10 + usize::from(target.effect_name_alt.is_some()) + tail);
            match (target.flip_axis.as_deref(), axis_slot.and_then(|slot| macro_site.args.get(slot)))
            {
                (Some(_), Some(_)) => {}
                (Some(_), None) => report.skipped.push(format!(
                    "{label}: `{}` has no measured flip-axis/control tail slot — source syncing left that value unchanged",
                    macro_site.name
                )),
                (None, _) => report.skipped.push(format!(
                    "{label}: `{}` flip-axis/control value was cleared — source syncing does not remove a macro tail",
                    macro_site.name
                )),
            }
        }
        edits.extend(transform_edits(text, macro_site, target));
        off_kind_edits(
            text,
            label,
            macro_site,
            differs[0],
            was,
            target,
            &call_sites,
            &pristine,
            &all_sites,
            &mut edits,
            &mut report,
        );

        let sites = modifier_sites.get(ordinal);
        modifier_edits(
            text,
            label,
            "rate",
            "LAST_EFFECT_SET_RATE",
            macro_site,
            was.rate.map(|v| vec![v]),
            target.rate.map(|v| vec![v]),
            sites.and_then(|s| s.rate.as_ref()),
            &mut edits,
            &mut report,
        );
        work_int_edits(
            text,
            label,
            macro_site,
            was.work_int.as_deref(),
            target.work_int.as_deref(),
            sites.and_then(|s| s.work_int.as_ref()),
            &mut edits,
            &mut report,
        );
        modifier_edits(
            text,
            label,
            "camera offset",
            "LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT",
            macro_site,
            was.camera_offset.map(|v| vec![v]),
            target.camera_offset.map(|v| vec![v]),
            sites.and_then(|s| s.camera_offset.as_ref()),
            &mut edits,
            &mut report,
        );
        modifier_edits(
            text,
            label,
            "tint",
            "LAST_EFFECT_SET_COLOR",
            macro_site,
            was.tint.map(|v| v.to_vec()),
            target.tint.map(|v| v.to_vec()),
            sites.and_then(|s| s.tint.as_ref()),
            &mut edits,
            &mut report,
        );
        modifier_edits(
            text,
            label,
            "particle tint",
            "LAST_PARTICLE_SET_COLOR",
            macro_site,
            was.particle_tint.map(|v| v.to_vec()),
            target.particle_tint.map(|v| v.to_vec()),
            sites.and_then(|s| s.particle_tint.as_ref()),
            &mut edits,
            &mut report,
        );
        modifier_edits(
            text,
            label,
            "opacity",
            "LAST_EFFECT_SET_ALPHA",
            macro_site,
            was.alpha.map(|v| vec![v]),
            target.alpha.map(|v| vec![v]),
            sites.and_then(|s| s.alpha.as_ref()),
            &mut edits,
            &mut report,
        );
        modifier_edits(
            text,
            label,
            "scale W",
            "LAST_EFFECT_SET_SCALE_W",
            macro_site,
            was.scale_w.clone(),
            target.scale_w.clone(),
            sites.and_then(|s| s.scale_w.as_ref()),
            &mut edits,
            &mut report,
        );
    }

    let value_count = edits.len();
    let (valued, map) = apply_mapped(text, edits.clone());
    let moved = if line_retimes.is_empty() {
        apply_effect_timing_moves(text, &timing_moves, &pristine, &ordinals, &sites, &edits)
    } else {
        let retimed: Option<Vec<crate::source_timing::EffectRetimed>> = line_retimes
            .iter()
            .map(|&index| {
                let ordinal = *ordinals.get(index)?;
                let spawn = sites.get(ordinal)?;
                let mut calls = vec![map(spawn.span.start)..map(spawn.span.end)];
                if let Some(modifiers) = modifier_sites.get(ordinal) {
                    let mut extra: Vec<&MacroSite> = [
                        &modifiers.rate,
                        &modifiers.work_int,
                        &modifiers.camera_offset,
                        &modifiers.tint,
                        &modifiers.particle_tint,
                        &modifiers.alpha,
                        &modifiers.scale_w,
                    ]
                    .into_iter()
                    .flatten()
                    .collect();
                    extra.sort_by_key(|site| site.span.start);
                    calls.extend(extra.iter().map(|site| map(site.span.start)..map(site.span.end)));
                }
                Some(crate::source_timing::EffectRetimed {
                    calls,
                    before: &pristine[index],
                    after: &edited[index],
                })
            })
            .collect();
        let result = retimed
            .ok_or_else(|| "a retimed effect has no source call".to_string())
            .and_then(|retimed| {
                let spawns: Vec<Range<usize>> =
                    sites.iter().map(|site| map(site.span.start)..map(site.span.end)).collect();
                crate::source_timing::retime_effects(&valued, &spawns, &edited, &retimed)
            });
        match result {
            Ok(updated) => {
                timing_moves = line_retimes
                    .iter()
                    .map(|&index| EffectTimingMove {
                        call_index: index,
                        new_start: edited[index].active_start,
                        new_end: edited[index].active_end,
                        move_start: pristine[index].active_start != edited[index].active_start,
                        move_end: pristine[index].active_end != edited[index].active_end,
                    })
                    .collect();
                Some(updated)
            }
            Err(reason) => {
                report.skipped.append(&mut timing_skipped);
                report.skipped.push(format!("{label}: the effect retime was not written — {reason}"));
                Some(valued.clone())
            }
        }
    };
    let (updated, moved_count) = match moved {
        Some(updated) => {
            let count = timing_moves
                .iter()
                .map(|timing| usize::from(timing.move_start) + usize::from(timing.move_end))
                .sum();
            (updated, count)
        }
        None => {
            report.skipped.push(format!(
                "{label}: the approved timing blocks could not be applied as one source edit"
            ));
            (valued.clone(), 0)
        }
    };
    report.changed = value_count + moved_count;
    // Insert added spawns the script never had. Each is a brand-new frame-block
    // entry (or an append to its frame's execute block), so there is no existing
    // line to retune — this is the path the old blanket refusal closed. A source
    // conditional, loop, or opaque statement makes the insertion context ambiguous;
    // leave it alone and explain why rather than placing a new spawn in the wrong arm.
    let mut updated = updated;
    let flat = effect_source_is_flat(&script.stmts);
    // The frame-block inserter below reads only `frame(…)`. A script stepped by `wait` — most of
    // them — goes through the timeline-following inserter instead, which is also what a retime
    // uses, so an added effect lands on the same frame either way.
    let mut additions_allowed = flat;
    if edited.len() > pristine.len() && !flat {
        let mut timed: Vec<crate::source_timing::EffectAddition> = Vec::new();
        let mut written = 0usize;
        let mut with_helpers = updated.clone();
        for call in edited.iter().skip(pristine.len()) {
            if call.disabled {
                report.skipped.push(format!(
                    "{label}: an added effect spawn is disabled — source syncing does not invent a disabled source call"
                ));
                continue;
            }
            // A copy of a spawn that sat in a condition carries that condition only as verbatim
            // context lines, not as a guard; which arm the copy belongs in is unknown.
            if call.leading.iter().chain(&call.trailing).any(|line| line.contains('{')) {
                report.skipped.push(format!(
                    "{label}: an added effect spawn copies one inside a condition in the source — source syncing cannot choose its execution context"
                ));
                continue;
            }
            let lines = crate::acmd::effect_call_insert_lines(call);
            if lines.is_empty() {
                report.skipped.push(format!(
                    "{label}: an added effect spawn has no source representation — source syncing left it unchanged"
                ));
                continue;
            }
            ensure_effect_helpers(&mut with_helpers, call);
            written += lines.len();
            timed.push(crate::source_timing::EffectAddition {
                frame: call.active_start,
                lines,
                guard: call.guard.clone(),
            });
            if let Some(stop) = crate::acmd::effect_call_stop_insert_line(call) {
                written += 1;
                timed.push(crate::source_timing::EffectAddition {
                    frame: call.active_end.max(call.active_start),
                    lines: vec![stop],
                    guard: None,
                });
            }
        }
        match crate::source_timing::add_effects(&with_helpers, &timed) {
            Ok(text) => {
                updated = text;
                report.changed += written;
                // The read-back below checks these exactly as it checks the flat path's.
                additions_allowed = true;
            }
            Err(reason) => report.skipped.push(format!(
                "{label}: the added effect spawns were not written — {reason}"
            )),
        }
    }
    if edited.len() > pristine.len() && flat {
        let mut additions: Vec<&crate::data::EffectCall> =
            edited.iter().skip(pristine.len()).collect();
        additions.sort_by_key(|call| call.active_start);
        for call in additions {
            if call.disabled {
                report.skipped.push(format!(
                    "{label}: an added effect spawn is disabled — source syncing does not invent a disabled source call"
                ));
                continue;
            }
            ensure_effect_helpers(&mut updated, call);
            let lines = crate::acmd::effect_call_insert_lines(call);
            if lines.is_empty() {
                report.skipped.push(format!(
                    "{label}: an added effect spawn has no source representation — source syncing left it unchanged"
                ));
                continue;
            }
            insert_effect_lines(
                &mut updated,
                call.active_start,
                &lines,
                call.guard.as_deref(),
            );
            report.changed += lines.len();
            if let Some(stop) = crate::acmd::effect_call_stop_insert_line(call) {
                insert_effect_lines(
                    &mut updated,
                    call.active_end.max(call.active_start),
                    std::slice::from_ref(&stop),
                    None,
                );
                report.changed += 1;
            }
        }
    }
    if edited.len() > pristine.len() && additions_allowed {
        let added: Vec<_> = edited
            .iter()
            .skip(pristine.len())
            .filter(|call| !call.disabled)
            .cloned()
            .collect();
        if !added.is_empty() {
            let reparsed = crate::acmd::parse_effect_script(&updated).to_effect_calls();
            let actual = effect_call_set_signature(&reparsed);
            let wanted = effect_call_set_signature(&added);
            // Count as well as compare signatures. A newly requested call can be byte-for-byte
            // identical to an existing one; a containment-only check would then call the old
            // call proof that the insertion happened, especially when another structural edit
            // already populated `report.skipped` and disabled the full-list check below.
            let expected_len = pristine.len() + added.len();
            if reparsed.len() != expected_len || !effect_signatures_contain(&actual, &wanted) {
                report.skipped.push(format!(
                    "{label}: the added effect source did not reparse to the requested call; the addition was not applied"
                ));
                report.changed = value_count;
                return Ok((valued, report));
            }
        }
    }
    if report.skipped.is_empty() {
        let reparsed = crate::acmd::parse_effect_script(&updated).to_effect_calls();
        // A line move re-wraps a spawn that sat in a condition, so the lines around it — kept on
        // each call as provenance, not as anything an export reads back — are its own now.
        let signature = |calls: &[crate::data::EffectCall]| {
            if line_retimes.is_empty() {
                return effect_call_set_signature(calls);
            }
            let bare: Vec<crate::data::EffectCall> = calls
                .iter()
                .cloned()
                .map(|mut call| {
                    // Structure moves with the spawn; what still acts on it must not.
                    call.leading.clear();
                    call.trailing
                        .retain(|line| crate::source_timing::touches_last_effect(line));
                    call
                })
                .collect();
            effect_call_set_signature(&bare)
        };
        if signature(&reparsed) != signature(&edited) {
            // Do not return a source move that failed its own semantic read-back. Value edits are
            // still safe to keep, but the timing operation is refused as one batch so a future
            // parser/source-shape change cannot leave only half of a retime on disk.
            report.skipped.push(format!(
                "{label}: the rewritten effect source did not reparse to the edited call set; timing changes were not applied"
            ));
            report.changed = value_count;
            return Ok((valued, report));
        }
    }
    Ok((updated, report))
}

/// `pub fn $name(index, fighter, move_name, pristine, edited)`: sync one family's edited
/// points into the project's `$prefix` function through `$rewrite`.
macro_rules! point_syncs {
    ($($name:ident: $prefix:literal, $rewrite:ident, $event:ident;)*) => {$(
        pub fn $name(
            index: &SourceIndex,
            fighter: &str,
            move_name: &str,
            pristine: &[crate::data::$event],
            edited: &[crate::data::$event],
        ) -> Result<SyncReport> {
            let script_name = crate::acmd::acmd_script_name($prefix, move_name);
            sync_script(index, fighter, &script_name, |body| {
                $rewrite(body, &format!("{fighter}/{move_name}"), pristine, edited)
            })
        }
    )*};
}

point_syncs! {
    sync_effect_calls: "effect", rewrite_effect_calls, EffectCall;
    sync_hitboxes: "game", rewrite_hitboxes, Hitbox;
    sync_attack_mods: "game", rewrite_attack_mods, AttackModState;
    sync_speed_ex: "game", rewrite_speed_ex, SetSpeedExEvent;
    sync_speed: "game", rewrite_speed, SetSpeedEvent;
    sync_add_speed_no_limit: "game", rewrite_add_speed_no_limit, AddSpeedNoLimitEvent;
    sync_correct: "game", rewrite_correct, CorrectEvent;
    sync_ft_catch_stop: "game", rewrite_ft_catch_stop, FtCatchStopEvent;
    sync_motion_module_set_rate: "game", rewrite_motion_module_set_rate, MotionModuleSetRateEvent;
    sync_clr_speed: "game", rewrite_clr_speed, ClrSpeedEvent;
    sync_change_kinetic: "game", rewrite_change_kinetic, ChangeKineticEvent;
    sync_kinetic_energy: "game", rewrite_kinetic_energy, KineticEnergyEvent;
    sync_work_flags: "game", rewrite_work_flags, WorkFlagEvent;
    sync_work_transition_terms: "game", rewrite_work_transition_terms, WorkTransitionTermEvent;
    sync_work_module_inc_int: "game", rewrite_work_module_inc_int, WorkModuleIncIntEvent;
    sync_work_module_sets: "game", rewrite_work_module_sets, WorkModuleSetEvent;
    sync_kinetic_add_speed: "game", rewrite_kinetic_add_speed, KineticAddSpeedEvent;
    sync_kinetic_clear_speed_all: "game", rewrite_kinetic_clear_speed_all, KineticClearSpeedAllEvent;
    sync_set_air: "game", rewrite_set_air, SetAirEvent;
    sync_reverse_lr: "game", rewrite_reverse_lr, ReverseLrEvent;
    sync_expressions: "expression", rewrite_expressions, ExpressionEvent;
}

/// Read one indexed script, hand its text to `rewrite`, and splice the result back in.
///
/// The rewrite only ever sees the function it is meant to change, so nothing else in the
/// file — other moves, imports, the install block — can be disturbed by it.
fn sync_script(
    index: &SourceIndex,
    fighter: &str,
    script_name: &str,
    rewrite: impl FnOnce(&str) -> Result<(String, SyncReport)>,
) -> Result<SyncReport> {
    // The index is a snapshot. A source generator, an editor, or an earlier sync pass may have
    // inserted bytes before this function since the caller built it. `text.get(site.span)` alone
    // is not enough validation: a stale span can still be in bounds and contain a different
    // function, which would make a successful report edit the wrong script. Rebuild from the
    // same root for every write so the site and its file text come from one snapshot.
    let current = SourceIndex::build(&index.root)
        .with_context(|| format!("indexing linked source at {}", index.root.display()))?;
    if let Some(conflict) = current.conflict(fighter, script_name) {
        bail!("{}", conflict.description());
    }
    let Some(site) = current.script(fighter, script_name) else {
        bail!("{fighter}: the project has no {script_name} to sync into");
    };
    // A forwarding wrapper's own span holds only the call. Its edits belong in the callee, and
    // only when that callee is this script's alone and was shown verbatim.
    let (file, span) = match &site.delegate {
        None => (&site.file, site.span.clone()),
        Some(delegate) => {
            if let Some(reason) = delegate.write_refusal(script_name, &site.function) {
                bail!("{reason}");
            }
            (&delegate.file, delegate.span.clone())
        }
    };
    let text =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let Some(body) = text.get(span.clone()) else {
        bail!(
            "{} changed on disk since it was indexed — rescan and try again",
            file.display()
        );
    };
    // A function whose agent is `fighter` has its `FT_MOTION_RATE(fighter, …)` read as the
    // agent's own; the rewriters must count the same calls the parser typed.
    let _agent = crate::acmd::agent_scope(body);
    let _consts = crate::project_consts::scope(std::sync::Arc::clone(&current.consts));
    let _macros = crate::hash_macros::scope(std::sync::Arc::new(current.hash_macros.clone()));
    let _helpers = crate::project_helpers::scope(std::sync::Arc::clone(&current.helpers));
    let (updated_body, mut report) = rewrite(body)?;
    let refused = crate::project_helpers::take_refused();
    if refused > 0 {
        report.skipped.push(format!(
            "{script_name}: {refused} edit(s) changed a value that one of the project's helper \
             functions fixes (such as the hidden argument of a RUMBLE_HIT helper) — change it in \
             the helper itself"
        ));
    }
    // Values written as project consts change at the const's definition, wherever that is.
    let pending = crate::project_consts::take_pending();
    if !pending.is_empty() {
        let (files, notes, changed) = crate::project_consts::apply_edits(&current.consts, &pending);
        report.changed += changed;
        report.skipped.extend(notes);
        for written in files {
            if !report.files.contains(&written) {
                report.files.push(written);
            }
        }
    }
    if updated_body == body {
        return Ok(report);
    }
    let mut updated = String::with_capacity(text.len());
    updated.push_str(&text[..span.start]);
    updated.push_str(&updated_body);
    updated.push_str(&text[span.end..]);
    std::fs::write(file, &updated).with_context(|| format!("writing {}", file.display()))?;
    report.files.push(file.clone());
    Ok(report)
}

/// Write one `LAST_EFFECT_SET_*` modifier back, or say why it could not be.
///
/// Each of these lives on its own line, so turning one on or off is a call added or removed —
/// structural, and reported. Only retuning an existing one is a value edit. The modifiers differ
/// only in which property they carry — rate, camera offset, opacity, effect tint, or
/// particle tint —
/// and every rule around them is the same. `noun` names the property in the user's terms and
/// `command` names the line to look for.
#[allow(clippy::too_many_arguments)]
fn modifier_edits(
    text: &str,
    label: &str,
    noun: &str,
    command: &str,
    macro_site: &MacroSite,
    was: Option<Vec<f32>>,
    now: Option<Vec<f32>>,
    site: Option<&MacroSite>,
    edits: &mut Vec<Replacement>,
    report: &mut SyncReport,
) {
    if was == now {
        return;
    }
    if command == "LAST_EFFECT_SET_SCALE_W"
        && site.is_some_and(|site| site.name == "visionary_last_effect_set_scale_w")
    {
        match (was.as_ref(), now.as_ref(), site) {
            (Some(_), Some(values), Some(site)) => {
                if let Some(span) = site.args.get(1) {
                    let current = text[span.clone()].trim();
                    let current_values = current
                        .strip_prefix("&[")
                        .and_then(|value| value.strip_suffix(']'))
                        .map(|value| {
                            value
                                .split(',')
                                .filter_map(|value| value.trim().parse::<f32>().ok())
                                .collect::<Vec<_>>()
                        });
                    if current_values.as_ref() != Some(values) {
                        edits.push(Replacement {
                            span: span.clone(),
                            value: format!(
                                "&[{}]",
                                values
                                    .iter()
                                    .map(|value| format_float(*value))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        });
                    }
                }
            }
            (None, Some(_), _) => report.skipped.push(format!(
                "{label}: `{}` gained a {noun} — that is a new {command} line, which source \
                 syncing does not add",
                macro_site.name
            )),
            (Some(_), None, _) => report.skipped.push(format!(
                "{label}: `{}` lost its {noun} — source syncing does not delete the {command} \
                 line that sets it",
                macro_site.name
            )),
            (Some(_), Some(_), None) => report.skipped.push(format!(
                "{label}: `{}` has a {noun} in the editor but no {command} directly beneath it \
                 in the source — reload the move from source",
                macro_site.name
            )),
            (None, None, _) => {}
        }
        return;
    }
    match (was, now, site) {
        (Some(_), Some(values), Some(site)) => {
            // Argument 0 is `agent`, so the components start at 1 and are in source order —
            // the one layout the whole family shares.
            for (offset, value) in values.iter().enumerate() {
                if let Some(span) = site.args.get(offset + 1) {
                    edits.extend(to_f32_edit(text, span, *value));
                }
            }
        }
        (None, Some(_), _) => report.skipped.push(format!(
            "{label}: `{}` gained a {noun} — that is a new {command} line, which source \
             syncing does not add",
            macro_site.name
        )),
        (Some(_), None, _) => report.skipped.push(format!(
            "{label}: `{}` lost its {noun} — source syncing does not delete the {command} \
             line that sets it",
            macro_site.name
        )),
        (Some(_), Some(_), None) => report.skipped.push(format!(
            "{label}: `{}` has a {noun} in the editor but no {command} directly beneath it \
             in the source — reload the move from source",
            macro_site.name
        )),
        (None, None, _) => {}
    }
}

/// Rewrite the authored WorkModule slot of an existing `LAST_EFFECT_SET_WORK_INT` line.
///
/// Unlike the numeric last-effect modifiers, this value is a source token rather than a float:
/// a named Work ID is meaningful only with its authored constant spelling. Adding or removing
/// the line remains structural, so source syncing only changes an existing argument.
#[allow(clippy::too_many_arguments)]
fn work_int_edits(
    text: &str,
    label: &str,
    macro_site: &MacroSite,
    was: Option<&str>,
    now: Option<&str>,
    site: Option<&MacroSite>,
    edits: &mut Vec<Replacement>,
    report: &mut SyncReport,
) {
    if was == now {
        return;
    }
    match (was, now, site) {
        (Some(_), Some(value), Some(site)) => {
            if let Some(edit) = site.args.get(1).and_then(|span| {
                let replacement = crate::acmd::const_expr(value);
                (text[span.clone()].trim() != replacement).then_some(Replacement {
                    span: span.clone(),
                    value: replacement,
                })
            }) {
                edits.push(edit);
            }
        }
        (None, Some(_), _) => report.skipped.push(format!(
            "{label}: `{}` gained a Work ID — that is a new LAST_EFFECT_SET_WORK_INT line, which source syncing does not add",
            macro_site.name
        )),
        (Some(_), None, _) => report.skipped.push(format!(
            "{label}: `{}` lost its Work ID — source syncing does not delete the LAST_EFFECT_SET_WORK_INT line that sets it",
            macro_site.name
        )),
        (Some(_), Some(_), None) => report.skipped.push(format!(
            "{label}: `{}` has a Work ID in the editor but no LAST_EFFECT_SET_WORK_INT line directly beneath it in the source — reload the move from source",
            macro_site.name
        )),
        (None, None, _) => {}
    }
}

/// The `LAST_EFFECT_SET_*` / `LAST_PARTICLE_SET_COLOR` lines a spawn can carry, in the spelling
/// `scan_macro_sites` reports.
///
/// Only these seven break nothing when they sit between a spawn and a later modifier of its
/// own; every other macro ends the run. Adding a member of the family to
/// [`crate::data::EffectCall`] means adding it here too, or its line will end the run and the
/// modifier after it will be reported as unfindable.
const MODIFIER_COMMANDS: &[&str] = &[
    "LAST_EFFECT_SET_RATE",
    "LAST_EFFECT_SET_WORK_INT",
    "LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT",
    "LAST_EFFECT_SET_COLOR",
    "LAST_PARTICLE_SET_COLOR",
    "LAST_EFFECT_SET_ALPHA",
    "LAST_EFFECT_SET_SCALE_W",
    "visionary_last_effect_set_scale_w",
];

/// The last-target modifier lines belonging to one spawn, each present only if the source has it.
#[derive(Default)]
struct ModifierSites {
    rate: Option<MacroSite>,
    work_int: Option<MacroSite>,
    camera_offset: Option<MacroSite>,
    tint: Option<MacroSite>,
    particle_tint: Option<MacroSite>,
    alpha: Option<MacroSite>,
    scale_w: Option<MacroSite>,
}

/// The spawn calls in `text`, in ordinal order, each paired with its `LAST_EFFECT_SET_*` lines.
///
/// These macros name no effect, so the only thing tying one to a spawn is that it comes
/// directly after it — the same rule `eval_effect_stmts` uses to fill `EffectCall::rate`,
/// `camera_offset`, `tint`, `particle_tint`, and `alpha`, and the two must agree or a value would
/// be read off one call and
/// written into another. Anything else between them, including a macro this scanner does not
/// recognise, breaks the pairing rather than reaching further back for a spawn to claim.
///
/// A modifier does *not* break the run for the modifiers after it: a script that writes a tint
/// and then a rate has both of them naming the spawn above the pair, and `eval_effect_stmts`
/// reads them that way too.
/// The spawn sites of `text`, in order, by name — the list `rewrite_effect_calls` indexes by
/// call ordinal. Exposed so a corpus oracle can check it against what the parser produces.
#[cfg(test)]
pub fn spawn_site_names(text: &str) -> Vec<String> {
    spawn_and_modifier_sites(text)
        .0
        .into_iter()
        .map(|site| site.name)
        .collect()
}

fn spawn_and_modifier_sites(text: &str) -> (Vec<MacroSite>, Vec<ModifierSites>) {
    let mut spawns: Vec<MacroSite> = Vec::new();
    let mut modifiers: Vec<ModifierSites> = Vec::new();
    let mut adjacent = false;
    let mut sites = scan_macro_sites(text, 0..text.len());
    sites.extend(scan_named_sites(
        text,
        "visionary_last_effect_set_scale_w",
        0..text.len(),
    ));
    sites.extend(native_effect_control_sites(text));
    sites.sort_by_key(|site| site.span.start);
    for site in sites {
        if is_native_effect_control_name(&site.name) {
            adjacent = false;
            if is_typed_native_effect_control(text, &site) {
                spawns.push(site);
                modifiers.push(ModifierSites::default());
            }
            continue;
        }
        if is_spawn_macro(&site.name) {
            // A trail is a spawn for ordinal purposes but never anchors a modifier, matching
            // the parser — see the `AfterImage` arm of `eval_effect_stmts`. A colour command is
            // counted for the same reason and anchors nothing for the same reason: both
            // produce an `EffectCall`, so both consume an ordinal, and neither is what
            // `LAST_EFFECT_SET_*` would find at runtime.
            adjacent = crate::acmd::is_effect_spawn_macro(&site.name);
            spawns.push(site);
            modifiers.push(ModifierSites::default());
            continue;
        }
        if !MODIFIER_COMMANDS.contains(&site.name.as_str()) {
            adjacent = false;
            continue;
        }
        if !adjacent {
            continue;
        }
        let Some(entry) = modifiers.last_mut() else {
            continue;
        };
        let valid_arity = match site.name.as_str() {
            "LAST_EFFECT_SET_COLOR" | "LAST_PARTICLE_SET_COLOR" => site.args.len() == 4,
            "LAST_EFFECT_SET_SCALE_W" => (2..=4).contains(&site.args.len()),
            "visionary_last_effect_set_scale_w" => site
                .args
                .get(1)
                .and_then(|span| text.get(span.clone()))
                .and_then(|value| value.trim().strip_prefix("&["))
                .and_then(|value| value.strip_suffix(']'))
                .map(|value| {
                    let values = value
                        .split(',')
                        .filter_map(|value| value.trim().parse::<f32>().ok())
                        .count();
                    (1..=3).contains(&values)
                })
                .unwrap_or(false),
            _ => site.args.len() == 2,
        };
        if !valid_arity {
            adjacent = false;
            continue;
        }
        // A second line of the same kind overwrites the first, because in game the later call
        // wins and that is the value the parser will have read.
        let slot = match site.name.as_str() {
            "LAST_EFFECT_SET_WORK_INT" => &mut entry.work_int,
            "LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT" => &mut entry.camera_offset,
            "LAST_EFFECT_SET_COLOR" => &mut entry.tint,
            "LAST_PARTICLE_SET_COLOR" => &mut entry.particle_tint,
            "LAST_EFFECT_SET_ALPHA" => &mut entry.alpha,
            "LAST_EFFECT_SET_SCALE_W" | "visionary_last_effect_set_scale_w" => &mut entry.scale_w,
            _ => &mut entry.rate,
        };
        *slot = Some(site);
    }
    (spawns, modifiers)
}

/// Whether a scanned macro name is one `call_macro_ordinals` counts.
///
/// "Spawn" is a slight lie now that colour commands are in the list, but the property that
/// matters is unchanged: these are exactly the macros that produce an `EffectCall`, so this
/// predicate and `call_macro_ordinals` must accept the same set or every ordinal past the
/// first disagreement points at the wrong line of source.
fn is_spawn_macro(name: &str) -> bool {
    crate::acmd::is_effect_spawn_macro(name)
        || is_trail_macro(name)
        || crate::data::is_color_command(name)
        || is_effect_control_macro(name)
}

fn is_effect_control_macro(name: &str) -> bool {
    matches!(
        name,
        "EFFECT_DETACH_KIND"
            | "EFFECT_DETACH_KIND_WORK"
            | "EFFECT_OFF_KIND_WORK"
            | "ENABLE_AREA"
            | "UNABLE_AREA"
    )
}

/// Whether a scanned macro name starts an AFTER_IMAGE trail.
///
/// Trails produce an `EffectCall` like any other spawn, so they have to be counted here to
/// keep call ordinals aligned with the source — but their arguments share none of the spawn
/// layout, so nothing may be written into them positionally.
fn is_trail_macro(name: &str) -> bool {
    name.starts_with("AFTER_IMAGE4_ON")
        || name == "AFTER_IMAGE_ON"
        // The raw-command trails, under the name `scan_raw_trail_site` gives them. These are the
        // only trail-ON calls that actually occur in the corpus.
        || crate::data::RAW_TRAIL_COMMANDS
            .iter()
            .any(|(_, site)| *site == name)
}

/// Everything a value rewrite CAN change about a call — the test for whether every iteration
/// of a loop body agrees, since they all come off one line of source.
///
/// The `LAST_EFFECT_SET_*` modifiers count: each is written back from its own line, but that
/// line is inside the loop body too, so a per-iteration rate, camera offset, effect tint, particle
/// tint, or opacity is no more expressible than a per-iteration position.
fn transform_matches(a: &crate::data::EffectCall, b: &crate::data::EffectCall) -> bool {
    a.offset == b.offset
        && a.rotation == b.rotation
        && a.scale == b.scale
        && a.flip_axis == b.flip_axis
        && a.rate == b.rate
        && a.work_int == b.work_int
        && a.camera_offset == b.camera_offset
        && a.tint == b.tint
        && a.particle_tint == b.particle_tint
        && a.alpha == b.alpha
        && a.scale_w == b.scale_w
        && a.color == b.color
        && a.control == b.control
}

/// Everything a value rewrite or a bounded timing move CANNOT change about a call.
///
/// The exact spawn command is structural: changing it can change the native tail ABI, whether a
/// second graphic exists, and whether the effect has a lifetime. The editor/export/live paths
/// support that change, while linked-source sync reports it rather than rewriting a macro into a
/// different signature by positional guesswork.
fn structural_identity_matches(a: &crate::data::EffectCall, b: &crate::data::EffectCall) -> bool {
    let same_control_kind = match (&a.control, &b.control) {
        (Some(left), Some(right)) => left.command_name() == right.command_name(),
        (None, None) => true,
        _ => false,
    };
    same_control_kind
        && a.effect_name.eq_ignore_ascii_case(&b.effect_name)
        && a.effect_name_alt == b.effect_name_alt
        && a.bone_name.eq_ignore_ascii_case(&b.bone_name)
        // A trail's second edge is a joint like the first, and no transform rewrite reaches it.
        && a.trail_bone2 == b.trail_bone2
        && a.spawn_func == b.spawn_func
        && a.trail_command == b.trail_command
        && a.disabled == b.disabled
}

/// Replacements for one spawn's position, rotation, and scale arguments.
fn transform_edits(
    text: &str,
    site: &MacroSite,
    call: &crate::data::EffectCall,
) -> Vec<Replacement> {
    // agent, graphic[, flipped graphic], joint, x, y, z, zr, yr, xr, size — the same layout
    // the parser reads, including the reversed rotation slots.
    let off = usize::from(call.effect_name_alt.is_some());
    let mut edits: Vec<Replacement> = Vec::new();
    let slots: [(usize, f32); 7] = [
        (3 + off, call.offset[0]),
        (4 + off, call.offset[1]),
        (5 + off, call.offset[2]),
        (6 + off, call.rotation[2]),
        (7 + off, call.rotation[1]),
        (8 + off, call.rotation[0]),
        (9 + off, call.scale),
    ];
    for (slot, value) in slots {
        if let Some(span) = site.args.get(slot) {
            edits.extend(float_edit(text, span, value));
        }
    }
    if let (Some(axis), Some(tail_index)) = (
        call.flip_axis.as_deref(),
        crate::acmd::effect_flip_axis_tail_index(&call.spawn_func),
    ) {
        if let Some(span) = site.args.get(10 + off + tail_index) {
            let current = text[span.clone()].trim();
            let token = axis.trim();
            let replacement = if token.parse::<i64>().is_ok() || token.starts_with('*') {
                token.to_string()
            } else {
                format!("*{token}")
            };
            if current != replacement {
                edits.push(Replacement {
                    span: span.clone(),
                    value: replacement,
                });
            }
        }
    }
    edits
}

/// Replacements for one colour command's arguments.
///
/// The layout comes from the command found in the *source*, not from what the editor holds:
/// if the two have come apart, the source is what the spans belong to, and writing a
/// transition length into a `FLASH` that has no such slot would put it in the red channel.
fn color_edits(text: &str, site: &MacroSite, color: &crate::data::ColorCall) -> Vec<Replacement> {
    let Some((has_transition, has_rgba)) = crate::data::color_command_layout(&site.name) else {
        return Vec::new();
    };
    let (transition_slot, rgba_slots) = crate::data::color_slots(has_transition);
    let mut slots: Vec<(usize, f32)> = Vec::new();
    if let (Some(slot), Some(value)) = (transition_slot, color.transition) {
        slots.push((slot, value));
    }
    if let (true, Some(rgba)) = (has_rgba, color.rgba) {
        slots.extend(rgba_slots.into_iter().zip(rgba));
    }
    slots
        .into_iter()
        .filter_map(|(slot, value)| to_f32_edit(text, site.args.get(slot)?, value))
        .collect()
}

/// Replacements for an existing C4 point command. The command itself is structural and is
/// therefore required to stay the same; its authored values are safe to retune in place.
fn control_edits(
    text: &str,
    label: &str,
    site: &MacroSite,
    was: &crate::data::EffectControl,
    now: &crate::data::EffectControl,
    edits: &mut Vec<Replacement>,
    report: &mut SyncReport,
) {
    if was.command_name() != now.command_name() {
        report.skipped.push(format!(
            "{label}: `{}` changed control command — source syncing only retunes existing values",
            site.name
        ));
        return;
    }
    match (was, now) {
        (
            crate::data::EffectControl::DetachKind { .. },
            crate::data::EffectControl::DetachKind { effect_name, unk },
        ) => {
            if let Some(edit) = control_const_edit(text, site, 1, effect_name, true) {
                edits.push(edit);
            }
            control_unk_edit(text, label, site, 2, *unk, edits, report);
        }
        (
            crate::data::EffectControl::DetachKindWork { .. },
            crate::data::EffectControl::DetachKindWork { work, unk },
        ) => {
            if let Some(edit) = control_const_edit(text, site, 1, work, false) {
                edits.push(edit);
            }
            control_unk_edit(text, label, site, 2, *unk, edits, report);
        }
        (
            crate::data::EffectControl::OffKindWork { preset_id: old_id },
            crate::data::EffectControl::OffKindWork { preset_id: new_id },
        ) if old_id != new_id => {
            // Slot identity and both flags are a closed source tuple, not editable scalar values.
            // Changing the preset through a source sync would rewrite structural native fields.
            report.skipped.push(format!(
                "{label}: `{}` changed off-kind Work preset — source syncing does not rewrite closed slot/flag tuples",
                site.name
            ));
        }
        (
            crate::data::EffectControl::EnableArea { .. },
            crate::data::EffectControl::EnableArea { kind },
        )
        | (
            crate::data::EffectControl::UnableArea { .. },
            crate::data::EffectControl::UnableArea { kind },
        ) => {
            if let Some(edit) = control_const_edit(text, site, 1, kind, false) {
                edits.push(edit);
            }
        }
        (
            crate::data::EffectControl::SetDisableRenderOffsetLast,
            crate::data::EffectControl::SetDisableRenderOffsetLast,
        )
        | (
            crate::data::EffectControl::EnableSyncInitPosLast,
            crate::data::EffectControl::EnableSyncInitPosLast,
        ) => {}
        _ => report.skipped.push(format!(
            "{label}: `{}` has a control payload that does not match its source command",
            site.name
        )),
    }
}

fn control_const_edit(
    text: &str,
    site: &MacroSite,
    slot: usize,
    value: &str,
    hash: bool,
) -> Option<Replacement> {
    let span = site.args.get(slot)?.clone();
    let replacement = if hash {
        crate::acmd::hash_arg(value)
    } else {
        crate::acmd::const_expr(value)
    };
    (text[span.clone()].trim() != replacement).then_some(Replacement {
        span,
        value: replacement,
    })
}

/// Write an edited `EFFECT_OFF_KIND` boolean pair back into the kill's own source line.
///
/// The stop is a separate call from the spawn it ends, so this cannot go through
/// [`transform_edits`]: it has to find the paired kill first, and it refuses rather than
/// guesses when the pairing is not unique — the same rule the timing mover already applies to
/// the same block.
#[allow(clippy::too_many_arguments)]
fn off_kind_edits(
    text: &str,
    label: &str,
    macro_site: &MacroSite,
    call_index: usize,
    was: &crate::data::EffectCall,
    target: &crate::data::EffectCall,
    call_sites: &[Option<MacroSite>],
    pristine: &[crate::data::EffectCall],
    all_sites: &[MacroSite],
    edits: &mut Vec<Replacement>,
    report: &mut SyncReport,
) {
    if was.off_fade == target.off_fade && was.off_detach == target.off_detach {
        return;
    }
    // "The editor has no value" is not a request to write the default one. A project saved
    // before these fields existed loads with both unset while the source it is synced against
    // has real values, and writing the default here would overwrite the author's own pair with
    // the invented one this whole change exists to stop.
    if target.off_fade.is_none() && target.off_detach.is_none() {
        return;
    }
    let (default_fade, default_detach) = crate::data::EFFECT_OFF_KIND_DEFAULT;
    if !target.follows_bone || target.active_end == crate::data::OPEN_ENDED_EFFECT_FRAME {
        report.skipped.push(format!(
            "{label}: `{}` changed its EFFECT_OFF_KIND flags but no longer ends at a finite \
             frame — there is no stop command in the source to retune",
            macro_site.name
        ));
        return;
    }
    let Some((stop_site, _)) = effect_stop_block(
        text, call_index, was, macro_site, call_sites, pristine, all_sites,
    ) else {
        report.skipped.push(format!(
            "{label}: `{}` changed its EFFECT_OFF_KIND flags but its stop command could not be \
             identified uniquely in the source",
            macro_site.name
        ));
        return;
    };
    for (slot, value) in [
        (2, target.off_fade.unwrap_or(default_fade)),
        (3, target.off_detach.unwrap_or(default_detach)),
    ] {
        let Some(span) = stop_site.args.get(slot) else {
            report.skipped.push(format!(
                "{label}: `EFFECT_OFF_KIND` has no argument {slot} to write"
            ));
            continue;
        };
        let current = text[span.clone()].trim();
        if current != "true" && current != "false" {
            report.skipped.push(format!(
                "{label}: `EFFECT_OFF_KIND` argument {slot} is not a literal boolean, so source \
                 syncing left it unchanged"
            ));
            continue;
        }
        if current != value.to_string() {
            edits.push(Replacement {
                span: span.clone(),
                value: value.to_string(),
            });
        }
    }
}

fn control_unk_edit(
    text: &str,
    label: &str,
    site: &MacroSite,
    slot: usize,
    value: i64,
    edits: &mut Vec<Replacement>,
    report: &mut SyncReport,
) {
    let Some(span) = site.args.get(slot) else {
        report.skipped.push(format!(
            "{label}: `{}` has no `unk` argument slot",
            site.name
        ));
        return;
    };
    let Some(edit) = int_edit(text, span, value) else {
        if text[span.clone()].trim().parse::<i64>().is_err() {
            report.skipped.push(format!(
                "{label}: `{}` has a non-literal `unk` argument, so source syncing left it unchanged",
                site.name
            ));
        }
        return;
    };
    edits.push(edit);
}

/// Rewrite one `game_*` function's source with the editor's edited hitbox values.
///
/// Pure, and the hitbox counterpart of [`rewrite_effect_calls`]. Same contract: existing
/// `ATTACK` arguments are retuned, and anything structural is reported instead.
pub fn rewrite_hitboxes(
    text: &str,
    label: &str,
    pristine: &[crate::data::Hitbox],
    edited: &[crate::data::Hitbox],
) -> Result<(String, SyncReport)> {
    // Each collision family is matched only against its own calls. They share the id space
    // but not the argument layout — slot 4 is damage in `ATTACK` and size in `CATCH` — so a
    // single candidate set meant a hitbox matched by id alone and had another family's values
    // written straight into it.
    let sites = scan_macro_sites(text, 0..text.len());
    let attacks: Vec<&MacroSite> = sites
        .iter()
        .filter(|s| crate::acmd::ATTACK_FUNCS.contains(&s.name.as_str()) || s.name == "ATTACK_FP")
        .collect();
    let catches: Vec<&MacroSite> = sites.iter().filter(|s| s.name == "CATCH").collect();
    let abs: Vec<&MacroSite> = sites.iter().filter(|s| s.name == "ATTACK_ABS").collect();
    // Name equality, not a prefix: `SET_SEARCH_SIZE_EXIST` is a two-argument modifier, and
    // retuning it through the 17-slot box layout is exactly the cross-family write this
    // per-family split exists to prevent.
    let searches: Vec<&MacroSite> = sites.iter().filter(|s| s.name == "SEARCH").collect();
    let winds: Vec<&MacroSite> = sites
        .iter()
        .filter(|s| crate::data::is_wind_command(&s.name))
        .collect();
    let positional_sites = positional_hitbox_sites(text, &sites, pristine);

    let mut report = SyncReport::default();
    let mut edits = Vec::new();
    // Ordinary hitboxes whose frames changed, moved once their values are written.
    let mut retimed: Vec<(usize, Range<usize>)> = Vec::new();
    // Hitboxes are keyed by id + part, which is what the game uses to tell them apart and
    // what stays stable across a retime. Two calls sharing a key in one script would be
    // ambiguous, so those are reported rather than guessed at.
    for (position, hitbox) in edited.iter().enumerate() {
        let Some(before) = pristine.get(position) else {
            report.skipped.push(format!(
                "{label}: hitbox {} was added — source syncing only retunes existing calls",
                hitbox.id
            ));
            continue;
        };
        if before == hitbox {
            continue;
        }
        // A wind area shares nothing with `ATTACK` but the id space, so it is matched against
        // its own calls and retuned through its own slot table.
        if hitbox.category == 2 || hitbox.wind.is_some() || before.wind.is_some() {
            edits.extend(wind_box_edits(
                text,
                label,
                &winds,
                before,
                hitbox,
                positional_sites.get(position).and_then(|site| *site),
                &mut report,
            ));
            continue;
        }
        // `ATTACK_ABS` shares the id space with nothing and is matched on its absolute kind:
        // every corpus call writes id 0, and kirby/ThrowF has two in one block that differ
        // only by kind. Matching on id here would be matching on a constant.
        if hitbox.category == crate::data::CAT_ABS || before.category == crate::data::CAT_ABS {
            let want = before.abs.as_ref().map(|a| a.kind.as_str()).unwrap_or("");
            let matching: Vec<&MacroSite> = positional_sites
                .get(position)
                .and_then(|site| *site)
                .filter(|site| site.name == "ATTACK_ABS")
                .map(|site| vec![site])
                .unwrap_or_else(|| {
                    abs.iter()
                        .copied()
                        .filter(|site| {
                            site.arg(text, 1).map(|a| a.trim().trim_start_matches('*'))
                                == Some(want)
                        })
                        .collect()
                });
            let [macro_site] = matching[..] else {
                report.skipped.push(format!(
                    "{label}: the {want} throw damage matches {} `ATTACK_ABS` calls in the \
                     source — cannot tell which one to retune",
                    matching.len()
                ));
                continue;
            };
            if before.active_start != hitbox.active_start || before.active_end != hitbox.active_end
            {
                retimed.push((position, macro_site.span.clone()));
            }
            let (call_edits, missing) = attack_abs_edits(text, macro_site, before, hitbox);
            if !missing.is_empty() {
                report.skipped.push(format!(
                    "{label}: the {want} throw damage changed {}, but its `ATTACK_ABS` call in \
                     the source is too short to have those arguments",
                    missing.join(", ")
                ));
            }
            edits.extend(call_edits);
            continue;
        }
        // `ATTACK_FP` is a collision, but its 41-slot payload is not the ordinary ATTACK
        // layout. Its source path has a separate table just as its parser and live hook do.
        if hitbox.category == crate::data::CAT_ATTACK_FP
            || before.category == crate::data::CAT_ATTACK_FP
        {
            if before.func != hitbox.func {
                report.skipped.push(format!(
                    "{label}: hitbox {} changed from `{}` to `{}` — source syncing rewrites \
                     argument values, not the macro being called",
                    before.id, before.func, hitbox.func
                ));
                continue;
            }
            if before.id != hitbox.id || before.part != hitbox.part {
                report.skipped.push(format!(
                    "{label}: ATTACK_FP hitbox {} was renumbered — source syncing only rewrites \
                     argument values",
                    before.id
                ));
                continue;
            }
            let matching: Vec<&MacroSite> = positional_sites
                .get(position)
                .and_then(|site| *site)
                .filter(|site| site.name == "ATTACK_FP")
                .map(|site| vec![site])
                .unwrap_or_else(|| {
                    sites
                        .iter()
                        .filter(|site| {
                            site.name == "ATTACK_FP"
                                && site_u32(text, site, 1)
                                    == Some(before.id)
                                && site_u32(text, site, 2)
                                    == Some(before.part)
                        })
                        .collect()
                });
            let [macro_site] = matching[..] else {
                report.skipped.push(format!(
                    "{label}: ATTACK_FP hitbox {} matches {} calls in the source — cannot tell \
                     which one to retune",
                    before.id,
                    matching.len()
                ));
                continue;
            };
            if before.active_start != hitbox.active_start || before.active_end != hitbox.active_end
            {
                retimed.push((position, macro_site.span.clone()));
            }
            let (call_edits, missing) = attack_fp_edits(text, macro_site, before, hitbox);
            if !missing.is_empty() {
                report.skipped.push(format!(
                    "{label}: ATTACK_FP hitbox {} changed {}, but its call in the source is too \
                     short to have those arguments",
                    before.id,
                    missing.join(", ")
                ));
            }
            edits.extend(call_edits);
            continue;
        }
        if before.id != hitbox.id || before.part != hitbox.part {
            report.skipped.push(format!(
                "{label}: hitbox {} was renumbered — source syncing only rewrites argument \
                 values",
                before.id
            ));
            continue;
        }
        // Swapping the family member is a different macro, not a different value. Rewriting
        // the call name in place would also have to add or drop the capsule triple, which is
        // structure — so it is reported, and the export path is where that swap can land.
        if before.func != hitbox.func {
            report.skipped.push(format!(
                "{label}: hitbox {} changed from `{}` to `{}` — source syncing rewrites \
                 argument values, not the macro being called",
                before.id, before.func, hitbox.func
            ));
            continue;
        }
        // `CATCH` takes no `part`, so a grab box is keyed on its id alone. `SEARCH` takes both,
        // and is keyed like an `ATTACK` — but against its own calls, since kirby/SpecialNStart
        // opens a `CATCH`, a `SEARCH` and an `ATTACK_ABS` all carrying id 0 in one block.
        let is_grab = hitbox.category == 1;
        let is_search = hitbox.category == crate::data::CAT_SEARCH;
        let matching: Vec<&MacroSite> = if let Some(site) = positional_sites
            .get(position)
            .and_then(|site| *site)
            .filter(|site| hitbox_site_matches(text, site, before))
        {
            vec![site]
        } else if is_grab {
            catches
                .iter()
                .copied()
                .filter(|site| {
                    site_u32(text, site, 1) == Some(before.id)
                })
                .collect()
        } else if is_search {
            searches
                .iter()
                .copied()
                .filter(|site| {
                    site_u32(text, site, 1) == Some(before.id)
                        && site_u32(text, site, 2)
                            == Some(before.part)
                })
                .collect()
        } else {
            attacks
                .iter()
                .copied()
                .filter(|site| {
                    // Family members share the id space but are not the same call, so an
                    // `ATTACK` hitbox never retunes an `ATTACK_IGNORE_THROW` beside it.
                    site.name == before.func
                        && site_u32(text, site, 1)
                            == Some(before.id)
                        && site_u32(text, site, 2)
                            == Some(before.part)
                })
                .collect()
        };
        let [macro_site] = matching[..] else {
            report.skipped.push(format!(
                "{label}: hitbox {} matches {} calls in the source — cannot tell which one to \
                 retune",
                before.id,
                matching.len()
            ));
            continue;
        };
        // A hitbox's frames are the `frame(...)` block it sits in and the clear that ends it,
        // not arguments of the call. It moves through [`crate::source_timing`] once its values
        // are written, which are written either way.
        if before.active_start != hitbox.active_start || before.active_end != hitbox.active_end {
            retimed.push((position, macro_site.span.clone()));
        }
        let (call_edits, missing) = if is_grab {
            catch_edits(text, macro_site, before, hitbox)
        } else if is_search {
            search_edits(text, macro_site, before, hitbox)
        } else {
            attack_edits(text, macro_site, before, hitbox)
        };
        if !missing.is_empty() {
            report.skipped.push(format!(
                "{label}: hitbox {} changed {}, but its `{}` call in the source is too short to \
                 have those arguments",
                before.id,
                missing.join(", "),
                macro_site.name
            ));
        }
        edits.extend(call_edits);
    }

    report.changed = edits.len();
    let (written, map) = apply_mapped(text, edits);
    if retimed.is_empty() {
        return Ok((written, report));
    }
    let moved: Vec<crate::source_timing::Retimed> = retimed
        .iter()
        .map(|(position, call)| crate::source_timing::Retimed {
            position: *position,
            call: map(call.start)..map(call.end),
            before: &pristine[*position],
            after: &edited[*position],
        })
        .collect();
    match crate::source_timing::retime_hitboxes(&written, pristine, &moved) {
        Ok(timed) => {
            report.changed += moved.len();
            Ok((timed, report))
        }
        Err(reason) => {
            let ids: Vec<String> = moved.iter().map(|m| m.before.id.to_string()).collect();
            report.skipped.push(format!(
                "{label}: hitbox {} was retimed, but its frames were left as they are — {reason}",
                ids.join(", ")
            ));
            Ok((written, report))
        }
    }
}

/// The new text for one changed `ATTACK` argument, by the kind of slot it is.
enum ArgValue {
    Float(f32),
    Int(i64),
    /// A number for a slot that is generic over `ToF32` — every `AREA_WIND_2ND*` argument is.
    /// Written the way the emitter writes one, `20` rather than `20.0`, so the two export paths
    /// put the same text in the file and a whole number does not sprout a decimal point.
    ToF32(f32),
    /// A const, a bool, a hash, or a capsule endpoint — compared and written as text.
    Text(String),
}

/// Replacements for the arguments of one `ATTACK` call that the user actually changed.
///
/// Returns the edits plus the names of any changed field whose slot the call does not have,
/// so a shortened `ATTACK` cannot swallow an edit silently.
///
/// Only *differing* fields are written. A blanket rewrite of every slot would churn calls
/// nobody touched, because the editor holds these values decoded: the parser turns a bare
/// `1` into `*ATTACK_LR_CHECK_F`, and writing the editor's spelling back unconditionally
/// would restyle every hitbox in the file on the first sync.
fn attack_edits(
    text: &str,
    site: &MacroSite,
    before: &crate::data::Hitbox,
    after: &crate::data::Hitbox,
) -> (Vec<Replacement>, Vec<&'static str>) {
    let bone = format!("Hash40::new(\"{}\")", after.bone_name.to_ascii_lowercase());
    // Live-captured hitboxes carry the collision attr as a raw hash — same rule as emit_attack.
    let collision_attr = match after.collision_attr.strip_prefix("0x") {
        Some(hex) => format!("Hash40::new_raw(0x{hex})"),
        None => format!("Hash40::new(\"{}\")", after.collision_attr),
    };
    let capsule = |axis: usize| match after.capsule_end {
        Some(end) => format!("Some({:.1})", end[axis]),
        None => "None".to_string(),
    };
    let konst = crate::acmd::const_expr;

    // Slots follow macros::ATTACK, the same numbering `parse_attack_call` documents and
    // `emit_attack` writes. Every argument the hitbox panels expose appears here, so an edit
    // either lands in the source or is named in the report — it can no longer just vanish.
    // Slots 1 and 2 (id, part) are absent on purpose: they identify which call this is, and
    // `rewrite_hitboxes` reports a renumber rather than matching by them and then changing
    // them out from under itself.
    let slots: [(usize, &'static str, bool, ArgValue); 34] = [
        (
            3,
            "bone",
            before.bone_name != after.bone_name,
            ArgValue::Text(bone),
        ),
        (
            4,
            "damage",
            before.damage != after.damage,
            ArgValue::Float(after.damage),
        ),
        (
            5,
            "angle",
            before.angle != after.angle,
            ArgValue::Int(after.angle as i64),
        ),
        (
            6,
            "knockback scaling",
            before.kb_scaling != after.kb_scaling,
            ArgValue::Int(after.kb_scaling as i64),
        ),
        (
            7,
            "fixed knockback",
            before.fkb != after.fkb,
            ArgValue::Int(after.fkb as i64),
        ),
        (
            8,
            "base knockback",
            before.kb_base != after.kb_base,
            ArgValue::Int(after.kb_base as i64),
        ),
        (
            9,
            "size",
            before.size != after.size,
            ArgValue::Float(after.size),
        ),
        (
            10,
            "x offset",
            before.offset_x != after.offset_x,
            ArgValue::Float(after.offset_x),
        ),
        (
            11,
            "y offset",
            before.offset_y != after.offset_y,
            ArgValue::Float(after.offset_y),
        ),
        (
            12,
            "z offset",
            before.offset_z != after.offset_z,
            ArgValue::Float(after.offset_z),
        ),
        (
            13,
            "capsule end",
            before.capsule_end != after.capsule_end,
            ArgValue::Text(capsule(0)),
        ),
        (
            14,
            "capsule end",
            before.capsule_end != after.capsule_end,
            ArgValue::Text(capsule(1)),
        ),
        (
            15,
            "capsule end",
            before.capsule_end != after.capsule_end,
            ArgValue::Text(capsule(2)),
        ),
        (
            16,
            "hitlag multiplier",
            before.hitlag_mult != after.hitlag_mult,
            ArgValue::Float(after.hitlag_mult),
        ),
        (
            17,
            "SDI multiplier",
            before.sdi_mult != after.sdi_mult,
            ArgValue::Float(after.sdi_mult),
        ),
        (
            18,
            "set-off kind",
            before.setoff_kind != after.setoff_kind,
            ArgValue::Text(konst(&after.setoff_kind)),
        ),
        (
            19,
            "facing check",
            before.lr_check != after.lr_check,
            ArgValue::Text(konst(&after.lr_check)),
        ),
        (
            20,
            "clang",
            before.is_clang != after.is_clang,
            ArgValue::Text(after.is_clang.to_string()),
        ),
        (
            21,
            "add attack",
            before.is_add_attack != after.is_add_attack,
            ArgValue::Int(after.is_add_attack as i64),
        ),
        (
            22,
            "hitbox attribute",
            before.hitbox_attr != after.hitbox_attr,
            ArgValue::Float(after.hitbox_attr),
        ),
        (
            23,
            "rehit",
            before.ground_or_air != after.ground_or_air,
            ArgValue::Int(after.ground_or_air as i64),
        ),
        (
            24,
            "meteor",
            before.is_mtk != after.is_mtk,
            ArgValue::Text(after.is_mtk.to_string()),
        ),
        (
            25,
            "shield disable",
            before.is_shield_disable != after.is_shield_disable,
            ArgValue::Text(after.is_shield_disable.to_string()),
        ),
        (
            26,
            "reflectable",
            before.is_reflectable != after.is_reflectable,
            ArgValue::Text(after.is_reflectable.to_string()),
        ),
        (
            27,
            "absorbable",
            before.is_absorbable != after.is_absorbable,
            ArgValue::Text(after.is_absorbable.to_string()),
        ),
        (
            28,
            "landing attack",
            before.is_landing_attack != after.is_landing_attack,
            ArgValue::Text(after.is_landing_attack.to_string()),
        ),
        (
            29,
            "situation mask",
            before.situation_mask != after.situation_mask,
            ArgValue::Text(konst(&after.situation_mask)),
        ),
        (
            30,
            "category mask",
            before.category_mask != after.category_mask,
            ArgValue::Text(konst(&after.category_mask)),
        ),
        (
            31,
            "part mask",
            before.part_mask != after.part_mask,
            ArgValue::Text(konst(&after.part_mask)),
        ),
        (
            32,
            "finish camera",
            before.no_finish_camera != after.no_finish_camera,
            ArgValue::Text(after.no_finish_camera.to_string()),
        ),
        (
            33,
            "collision attribute",
            before.collision_attr != after.collision_attr,
            ArgValue::Text(collision_attr),
        ),
        (
            34,
            "sound level",
            before.sound_level != after.sound_level,
            ArgValue::Text(konst(&after.sound_level)),
        ),
        (
            35,
            "sound attribute",
            before.sound_attr != after.sound_attr,
            ArgValue::Text(konst(&after.sound_attr)),
        ),
        (
            36,
            "attack region",
            before.attack_region != after.attack_region,
            ArgValue::Text(konst(&after.attack_region)),
        ),
    ];

    apply_slots(text, site, &shift_past_absent_capsule(text, site, slots))
}

/// Replacements for the established editor fields in one `ATTACK_FP` call.
///
/// The site indices include `agent`; the FP slots after it are independently documented in the
/// `smash-script` wrapper. Geometry and the remaining unknown fields are intentionally absent:
/// source syncing must not turn an ordinary ATTACK interpretation into a guess about FP.
fn attack_fp_edits(
    text: &str,
    site: &MacroSite,
    before: &crate::data::Hitbox,
    after: &crate::data::Hitbox,
) -> (Vec<Replacement>, Vec<&'static str>) {
    let attr = match after.collision_attr.strip_prefix("0x") {
        Some(hex) => format!("Hash40::new_raw(0x{hex})"),
        None => format!("Hash40::new(\"{}\")", after.collision_attr),
    };
    let konst = crate::acmd::const_expr;
    let slots: [(usize, &'static str, bool, ArgValue); 12] = [
        (
            4,
            "damage",
            before.damage != after.damage,
            ArgValue::Float(after.damage),
        ),
        (
            5,
            "angle",
            before.angle != after.angle,
            ArgValue::Int(after.angle as i64),
        ),
        (
            6,
            "knockback scaling",
            before.kb_scaling != after.kb_scaling,
            ArgValue::Int(after.kb_scaling as i64),
        ),
        (
            7,
            "fixed knockback",
            before.fkb != after.fkb,
            ArgValue::Int(after.fkb as i64),
        ),
        (
            8,
            "base knockback",
            before.kb_base != after.kb_base,
            ArgValue::Int(after.kb_base as i64),
        ),
        (
            13,
            "collision attribute",
            before.collision_attr != after.collision_attr,
            ArgValue::Text(attr),
        ),
        (
            15,
            "hitlag multiplier",
            before.hitlag_mult != after.hitlag_mult,
            ArgValue::Float(after.hitlag_mult),
        ),
        (
            16,
            "SDI multiplier",
            before.sdi_mult != after.sdi_mult,
            ArgValue::Float(after.sdi_mult),
        ),
        (
            17,
            "clang",
            before.is_clang != after.is_clang,
            ArgValue::Text(after.is_clang.to_string()),
        ),
        (
            22,
            "rehit",
            before.ground_or_air != after.ground_or_air,
            ArgValue::Int(after.ground_or_air as i64),
        ),
        (
            30,
            "reflectable",
            before.is_reflectable != after.is_reflectable,
            ArgValue::Text(after.is_reflectable.to_string()),
        ),
        (
            31,
            "absorbable",
            before.is_absorbable != after.is_absorbable,
            ArgValue::Text(after.is_absorbable.to_string()),
        ),
    ];
    let (mut edits, mut missing) = apply_slots(text, site, &slots);
    let tail: [(usize, &'static str, bool, ArgValue); 4] = [
        (
            20,
            "sound level",
            before.sound_level != after.sound_level,
            ArgValue::Text(konst(&after.sound_level)),
        ),
        (
            21,
            "sound attribute",
            before.sound_attr != after.sound_attr,
            ArgValue::Text(konst(&after.sound_attr)),
        ),
        (
            24,
            "attack region",
            before.attack_region != after.attack_region,
            ArgValue::Text(konst(&after.attack_region)),
        ),
        (
            35,
            "facing check",
            before.lr_check != after.lr_check,
            ArgValue::Text(konst(&after.lr_check)),
        ),
    ];
    let (tail_edits, tail_missing) = apply_slots(text, site, &tail);
    edits.extend(tail_edits);
    missing.extend(tail_missing);
    missing.sort_unstable();
    missing.dedup();
    (edits, missing)
}

/// Re-aim an `ATTACK` slot table at a call written without the optional capsule triple.
///
/// The archive writes `ATTACK_IGNORE_THROW` with 33 arguments where `ATTACK` has 36; every
/// slot past the transform then sits three earlier. Without this, retuning such a call wrote
/// the hitlag multiplier into `z`, the setoff kind into hitlag, and so on down the line —
/// well-formed source, silently the wrong move.
///
/// The three capsule slots are aimed past the end of the call instead of being dropped, so a
/// capsule edit on a call that has nowhere to put one is *reported* by `apply_slots` rather
/// than discarded.
fn shift_past_absent_capsule<const N: usize>(
    text: &str,
    site: &MacroSite,
    slots: [(usize, &'static str, bool, ArgValue); N],
) -> Vec<(usize, &'static str, bool, ArgValue)> {
    let optionish = |index: usize| {
        site.arg(text, index)
            .map(|a| a.trim())
            .is_some_and(|a| a == "None" || a.starts_with("Some("))
    };
    if site.args.len() >= 16 && (13..16).all(optionish) {
        return slots.into_iter().collect();
    }
    slots
        .into_iter()
        .map(|(slot, field, changed, value)| match slot {
            13..=15 => (usize::MAX, field, changed, value),
            16.. => (slot - 3, field, changed, value),
            _ => (slot, field, changed, value),
        })
        .collect()
}

/// Write each changed slot, and name any whose argument the call does not have.
///
/// A call shorter than its slot table is not an error — scripts do use the shorter macro
/// forms — but an edit that lands nowhere has to be reported rather than dropped.
fn apply_slots(
    text: &str,
    site: &MacroSite,
    slots: &[(usize, &'static str, bool, ArgValue)],
) -> (Vec<Replacement>, Vec<&'static str>) {
    let mut edits = Vec::new();
    let mut missing = Vec::new();
    for (slot, field, changed, value) in slots {
        if !changed {
            continue;
        }
        let Some(span) = site.args.get(*slot) else {
            if !missing.contains(field) {
                missing.push(*field);
            }
            continue;
        };
        let edit = match value {
            ArgValue::Float(v) => float_edit(text, span, *v),
            ArgValue::Int(v) => int_edit(text, span, *v),
            ArgValue::ToF32(v) => to_f32_edit(text, span, *v),
            ArgValue::Text(v) => text_edit(text, span, v),
        };
        edits.extend(edit);
    }
    (edits, missing)
}

/// Replacements for the arguments of one `SEARCH` call that the user actually changed.
///
/// A detection box's editable properties are its geometry, what it looks for, which hurtbox
/// states it counts, and its three trailing masks. The two undocumented slots have no control
/// and are never written; see [`crate::data::SearchExtras`].
///
/// The tail moves. `SEARCH` is dumped both with and without its three capsule arguments, and 4
/// of the corpus's 7 calls are the short form, so every argument after the geometry sits three
/// slots earlier in one shape than the other. Locating them by a fixed number would write the
/// situation mask over the hit status — a call that still compiles and detects the wrong thing,
/// which is the failure mode this file exists to refuse.
fn search_edits(
    text: &str,
    site: &MacroSite,
    before: &crate::data::Hitbox,
    after: &crate::data::Hitbox,
) -> (Vec<Replacement>, Vec<&'static str>) {
    let bone = format!("Hash40::new(\"{}\")", after.bone_name.to_ascii_lowercase());
    let capsule = |axis: usize| match after.capsule_end {
        Some(end) => format!("Some({:.1})", end[axis]),
        None => "None".to_string(),
    };
    let has_capsule_slots = site.arg(text, 8).is_some_and(crate::acmd::is_capsule_slot);
    let capsule_changed = before.capsule_end != after.capsule_end;
    let capsule_writable = capsule_changed && has_capsule_slots;
    let tail = if has_capsule_slots { 11 } else { 8 };
    let konst = |slot: usize, field: &'static str, was: &str, now: &str| {
        (
            slot,
            field,
            was != now,
            ArgValue::Text(crate::acmd::const_expr(now)),
        )
    };
    // A box with no extras at all can only come from a capture; fall back to the same stand-ins
    // the export uses, so "unchanged" means the same thing on both paths.
    let extras = |hb: &crate::data::Hitbox| {
        hb.search
            .clone()
            .unwrap_or_else(|| crate::data::Hitbox::default().to_search_call().extras)
    };
    let (was, now) = (extras(before), extras(after));
    let slots: [(usize, &'static str, bool, ArgValue); 14] = [
        (
            3,
            "bone",
            before.bone_name != after.bone_name,
            ArgValue::Text(bone),
        ),
        (
            4,
            "size",
            before.size != after.size,
            ArgValue::Float(after.size),
        ),
        (
            5,
            "x offset",
            before.offset_x != after.offset_x,
            ArgValue::Float(after.offset_x),
        ),
        (
            6,
            "y offset",
            before.offset_y != after.offset_y,
            ArgValue::Float(after.offset_y),
        ),
        (
            7,
            "z offset",
            before.offset_z != after.offset_z,
            ArgValue::Float(after.offset_z),
        ),
        (
            8,
            "capsule end",
            capsule_writable,
            ArgValue::Text(capsule(0)),
        ),
        (
            9,
            "capsule end",
            capsule_writable,
            ArgValue::Text(capsule(1)),
        ),
        (
            10,
            "capsule end",
            capsule_writable,
            ArgValue::Text(capsule(2)),
        ),
        konst(
            tail,
            "collision kind",
            &was.collision_kind,
            &now.collision_kind,
        ),
        konst(tail + 1, "hit status", &was.hit_status, &now.hit_status),
        konst(
            tail + 3,
            "situation mask",
            &before.situation_mask,
            &after.situation_mask,
        ),
        konst(
            tail + 4,
            "category mask",
            &before.category_mask,
            &after.category_mask,
        ),
        konst(tail + 5, "part mask", &before.part_mask, &after.part_mask),
        (
            1,
            "id",
            before.id != after.id,
            ArgValue::Int(after.id as i64),
        ),
    ];
    let (edits, mut missing) = apply_slots(text, site, &slots);
    // Same refusal `catch_edits` makes: adding a capsule to a call written without the slots
    // needs three arguments inserted, which a slot rewrite cannot do. Writing them anyway
    // would put `Some(1.0)` over `*COLLISION_KIND_MASK_ATTACK`.
    if capsule_changed && !has_capsule_slots && !missing.contains(&"capsule end") {
        missing.push("capsule end");
    }
    (edits, missing)
}

/// Replacements for the arguments of one `ATTACK_ABS` call that the user actually changed.
///
/// Its own table, and the slot numbers are *not* `ATTACK`'s. The two families name many of the
/// same properties — damage, angle, the knockback triple, hitlag, `lr_check`, the sound pair,
/// `attack_region` — in a different order and a different count, which is exactly the shape of
/// mistake that corrupts a call while still compiling.
///
/// Full layout (slot 0 is `agent`): 1 kind, 2 id, 3 damage, 4 angle, 5 kbg, 6 fkb, 7 bkb,
/// 8 hitlag, 9 unk, 10 lr_check, 11 unk2, 12 unk3, 13 collision_attr, 14 sound level, 15 sound
/// attr, 16 attack region. Slots 9, 11 and 12 are never written: they are invariant across the
/// corpus, undocumented, and the editor exposes no control for them.
fn attack_abs_edits(
    text: &str,
    site: &MacroSite,
    before: &crate::data::Hitbox,
    after: &crate::data::Hitbox,
) -> (Vec<Replacement>, Vec<&'static str>) {
    let konst = |slot: usize, field: &'static str, was: &str, now: &str| {
        (
            slot,
            field,
            was != now,
            ArgValue::Text(crate::acmd::const_expr(now)),
        )
    };
    let kind =
        |hb: &crate::data::Hitbox| hb.abs.as_ref().map(|a| a.kind.clone()).unwrap_or_default();
    let slots: [(usize, &'static str, bool, ArgValue); 13] = [
        konst(1, "absolute kind", &kind(before), &kind(after)),
        (
            2,
            "id",
            before.id != after.id,
            ArgValue::Int(after.id as i64),
        ),
        (
            3,
            "damage",
            before.damage != after.damage,
            ArgValue::Float(after.damage),
        ),
        (
            4,
            "angle",
            before.angle != after.angle,
            ArgValue::Int(after.angle as i64),
        ),
        (
            5,
            "knockback scaling",
            before.kb_scaling != after.kb_scaling,
            ArgValue::Int(after.kb_scaling as i64),
        ),
        (
            6,
            "fixed knockback",
            before.fkb != after.fkb,
            ArgValue::Int(after.fkb as i64),
        ),
        (
            7,
            "base knockback",
            before.kb_base != after.kb_base,
            ArgValue::Int(after.kb_base as i64),
        ),
        (
            8,
            "hitlag multiplier",
            before.hitlag_mult != after.hitlag_mult,
            ArgValue::Float(after.hitlag_mult),
        ),
        konst(10, "LR check", &before.lr_check, &after.lr_check),
        (
            13,
            "collision attribute",
            before.collision_attr != after.collision_attr,
            ArgValue::Text(format!("Hash40::new(\"{}\")", after.collision_attr)),
        ),
        konst(14, "sound level", &before.sound_level, &after.sound_level),
        konst(15, "sound attribute", &before.sound_attr, &after.sound_attr),
        konst(
            16,
            "attack region",
            &before.attack_region,
            &after.attack_region,
        ),
    ];
    apply_slots(text, site, &slots)
}

/// Replacements for the arguments of one `CATCH` call that the user actually changed.
///
/// `CATCH` numbers its arguments differently from `ATTACK` and has no `part`, no damage, and
/// no knockback — a grab box's editable properties are its joint, its size, its offsets, and
/// its capsule endpoint. Status and situation are not editable, so they are never written.
///
/// Slots 0..=6 mean the same thing in both of `CATCH`'s written shapes, so only the capsule
/// needs the form test — see [`crate::acmd::is_capsule_slot`]. In a call written without the
/// capsule arguments, slots 7 and 8 hold the status kind and the situation mask, and writing
/// `Some(1.0)` into them would both destroy the grab's behaviour and produce a file that does
/// not compile. Adding a capsule there needs three arguments *inserted*, which a slot rewrite
/// cannot do, so the change is reported instead — the same choice retimes get.
fn catch_edits(
    text: &str,
    site: &MacroSite,
    before: &crate::data::Hitbox,
    after: &crate::data::Hitbox,
) -> (Vec<Replacement>, Vec<&'static str>) {
    let bone = format!("Hash40::new(\"{}\")", after.bone_name.to_ascii_lowercase());
    let capsule = |axis: usize| match after.capsule_end {
        Some(end) => format!("Some({:.1})", end[axis]),
        None => "None".to_string(),
    };
    let has_capsule_slots = site.arg(text, 7).is_some_and(crate::acmd::is_capsule_slot);
    let capsule_changed = before.capsule_end != after.capsule_end;
    let capsule_writable = capsule_changed && has_capsule_slots;
    // agent, id, bone, size, x, y, z, x2, y2, z2, status, situation.
    let slots: [(usize, &'static str, bool, ArgValue); 8] = [
        (
            2,
            "bone",
            before.bone_name != after.bone_name,
            ArgValue::Text(bone),
        ),
        (
            3,
            "size",
            before.size != after.size,
            ArgValue::Float(after.size),
        ),
        (
            4,
            "x offset",
            before.offset_x != after.offset_x,
            ArgValue::Float(after.offset_x),
        ),
        (
            5,
            "y offset",
            before.offset_y != after.offset_y,
            ArgValue::Float(after.offset_y),
        ),
        (
            6,
            "z offset",
            before.offset_z != after.offset_z,
            ArgValue::Float(after.offset_z),
        ),
        (
            7,
            "capsule end",
            capsule_writable,
            ArgValue::Text(capsule(0)),
        ),
        (
            8,
            "capsule end",
            capsule_writable,
            ArgValue::Text(capsule(1)),
        ),
        (
            9,
            "capsule end",
            capsule_writable,
            ArgValue::Text(capsule(2)),
        ),
    ];
    let (edits, mut missing) = apply_slots(text, site, &slots);
    if capsule_changed && !has_capsule_slots && !missing.contains(&"capsule end") {
        missing.push("capsule end");
    }
    (edits, missing)
}

/// Match one wind area to its `AREA_WIND_2ND*` call and retune the arguments that changed.
///
/// Wind is a family in the strongest sense. Every argument is a bare float, so unlike `ATTACK`
/// there is no argument *shape* to recognise a layout from — the command name is the layout.
/// The four commands share slots 0..=7 and nothing else, so a rectangular call is only ever
/// matched against a rectangular one, and a width is never written where a radius goes.
///
/// Anything that is not a value change to an existing argument — a different command, a
/// different argument count, a renumber — is pushed to `report` and nothing is written.
fn wind_box_edits(
    text: &str,
    label: &str,
    winds: &[&MacroSite],
    before: &crate::data::Hitbox,
    after: &crate::data::Hitbox,
    positional_site: Option<&MacroSite>,
    report: &mut SyncReport,
) -> Vec<Replacement> {
    let (Some(was), Some(now)) = (before.wind.as_ref(), after.wind.as_ref()) else {
        report.skipped.push(format!(
            "{label}: wind box {} carries no `AREA_WIND` payload to retune — fetch the move again",
            before.id
        ));
        return Vec::new();
    };
    // The command is the layout, so swapping it is a different call, not a different value.
    if was.command != now.command || was.args.len() != now.args.len() {
        report.skipped.push(format!(
            "{label}: wind box {} changed from `{}` to `{}` — source syncing rewrites argument \
             values, not the macro being called",
            before.id, was.command, now.command
        ));
        return Vec::new();
    }
    if before.id != after.id || was.id() != now.id() {
        report.skipped.push(format!(
            "{label}: wind box {} was renumbered — source syncing only rewrites argument values",
            before.id
        ));
        return Vec::new();
    }
    let matching: Vec<&MacroSite> = positional_site
        .filter(|site| site.name == was.command)
        .map(|site| vec![site])
        .unwrap_or_else(|| {
            winds
                .iter()
                .copied()
                .filter(|site| {
                    site.name == was.command
                        && site
                            .arg(text, 1)
                            .and_then(|a| a.trim().parse::<f32>().ok())
                            .map(|id| id.max(0.0) as u32)
                            == Some(was.id())
                })
                .collect()
        });
    let [site] = matching[..] else {
        report.skipped.push(format!(
            "{label}: wind box {} matches {} `{}` calls in the source — cannot tell which one to \
             retune",
            before.id,
            matching.len(),
            was.command
        ));
        return Vec::new();
    };
    // `agent` occupies argument 0, so the call must be exactly one longer than the payload.
    // A shorter or longer one is a call this editor does not understand; retuning it by
    // position would write into whatever the author actually put there.
    if site.args.len() != was.args.len() + 1 {
        report.skipped.push(format!(
            "{label}: the `{}` call for wind box {} has {} arguments, not the {} that command \
             takes — source syncing will not retune it",
            was.command,
            before.id,
            site.args.len().saturating_sub(1),
            was.args.len()
        ));
        return Vec::new();
    }
    // A wind area's start frame is the block it sits in, and is never an argument. Its end
    // frame is the lifetime argument when the command has that slot — which this does rewrite,
    // so an end the payload now accounts for is a value edit and not a retime. The shorter
    // forms have no such slot and are ended by an `AreaModule::erase_wind` on another line, so
    // there the only end that can be written is the one already there.
    let end_moved = if now.has_lifetime() {
        after.active_end != now.end_frame(after.active_start)
    } else {
        after.active_end != before.active_end
    };
    if before.active_start != after.active_start || end_moved {
        report.skipped.push(format!(
            "{label}: wind box {} was retimed — its start is the block it sits in and its end is \
             an `erase_wind`, so source syncing cannot move it",
            before.id
        ));
    }
    let names: &[&'static str] = if now.is_radial() {
        &[
            "id",
            "strength",
            "radial falloff",
            "speed limit",
            "acceleration",
            "x offset",
            "y offset",
            "radius",
            "lifetime",
        ]
    } else {
        &[
            "id",
            "strength",
            "direction",
            "speed limit",
            "acceleration",
            "x offset",
            "y offset",
            "width",
            "height",
            "lifetime",
        ]
    };
    let slots: Vec<(usize, &'static str, bool, ArgValue)> = now
        .args
        .iter()
        .enumerate()
        .map(|(index, value)| {
            (
                index + 1,
                names.get(index).copied().unwrap_or("argument"),
                was.args.get(index) != Some(value),
                ArgValue::ToF32(*value),
            )
        })
        .collect();
    let (edits, missing) = apply_slots(text, site, &slots);
    if !missing.is_empty() {
        report.skipped.push(format!(
            "{label}: wind box {} changed {}, but its `{}` call in the source is too short to \
             have those arguments",
            before.id,
            missing.join(", "),
            was.command
        ));
    }
    edits
}

/// Replace a non-numeric argument — a const, a bool, a hash, a capsule endpoint — when the
/// text the editor would write differs from what is already there.
fn text_edit(text: &str, span: &Range<usize>, new: &str) -> Option<Replacement> {
    let current = text[span.clone()].trim();
    // A hash the author wrote through one of the project's own macros (`rb!("attackm")`) keeps
    // that spelling when the editor hands back a plain `Hash40::new("…")` for it.
    let macros = crate::hash_macros::current();
    let new = match new
        .strip_prefix("Hash40::new(\"")
        .and_then(|rest| rest.strip_suffix("\")"))
    {
        Some(name) if macros.expand_call(current).is_some() => {
            if macros.expand_call(current).as_deref() == Some(name) {
                return None;
            }
            macros.spell(current, name, text)
        }
        _ => new.to_string(),
    };
    (current != new).then(|| Replacement {
        span: span.clone(),
        value: new,
    })
}

/// The shared walk behind the direct point-family source rewrites.
///
/// Leaves the script alone when its buildable `sites` and the editor's events disagree in
/// count, and skips any changed event whose site is out of flat source order or that was
/// retimed. `edit` checks one changed event's site shape and emits its replacements.
/// `shape` names the calls in the count and per-site messages, says what source syncing
/// retunes instead of retiming, and carries the sites.
fn rewrite_points<C: PartialEq>(
    text: &str,
    label: &str,
    (count_noun, site_noun, retunes, sites): (&str, &str, &str, Vec<MacroSite>),
    pristine: &[crate::data::PointEvent<C>],
    edited: &[crate::data::PointEvent<C>],
    mut edit: impl FnMut(
        &MacroSite,
        &crate::data::PointEvent<C>,
        &crate::data::PointEvent<C>,
        &mut SyncReport,
        &mut Vec<Replacement>,
    ),
) -> Result<(String, SyncReport)> {
    let mut report = SyncReport::default();
    if pristine.len() != sites.len() || edited.len() != pristine.len() {
        report.skipped.push(format!(
            "{label}: source has {} buildable {count_noun} call(s), while the editor has {} pristine and {} edited point(s) — malformed or looped calls are source-only",
            sites.len(),
            pristine.len(),
            edited.len()
        ));
        return Ok((text.to_string(), report));
    }

    let mut edits = Vec::new();
    for (index, (before, now)) in pristine.iter().zip(edited).enumerate() {
        if before == now {
            continue;
        }
        if before.site != index || now.site != index {
            report.skipped.push(format!(
                "{label}: {site_noun} site {} does not match the flat source order — source syncing refuses positional guessing",
                before.site
            ));
            continue;
        }
        if before.frame != now.frame {
            report.skipped.push(format!(
                "{label}: {site_noun} site {} was retimed from frame {} to {} — source syncing only {retunes}",
                before.site, before.frame, now.frame
            ));
            continue;
        }
        let Some(site) = sites.get(index) else {
            continue;
        };
        edit(site, before, now, &mut report, &mut edits);
    }
    report.changed = edits.len();
    Ok((apply(text, edits), report))
}

// ── Hurtbox state write-back ─────────────────────────────────────────────────

/// The hurtbox commands, with the argument count each one has after `agent`.
///
/// The arity is carried here because it is part of *identifying* the call, not just of reading
/// it: the parser refuses a member written with the wrong number of arguments and leaves it as
/// a raw line, so a scanner that counted it anyway would number every site after it one too
/// high and retune the wrong call.
const HURT_COMMANDS: &[(&str, usize)] = &[
    ("HIT_NODE", 2),
    ("HIT_NO", 2),
    // One argument, not two: `WHOLE_HIT`'s target is the macro name. Listing it at 2 here would
    // exclude every real call from `hurt_sites` and shift every later site's ordinal.
    ("WHOLE_HIT", 1),
    ("HIT_RESET_ALL", 0),
    ("COL_PRI", 1),
    ("COL_NORMAL", 0),
];

/// The hurtbox calls in `text`, in document order — index `n` is site `n`.
///
/// This is the write-back's half of the correspondence [`crate::data::HurtSite`] defines. It
/// holds because a pre-order walk of the parsed script visits `is_excute` blocks, and the
/// statements inside them, in the order they appear in the file.
fn hurt_sites(text: &str) -> Vec<MacroSite> {
    scan_macro_sites(text, 0..text.len())
        .into_iter()
        .filter(|site| {
            HURT_COMMANDS
                .iter()
                .any(|(name, arity)| site.name == *name && site.args.len() == arity + 1)
        })
        .collect()
}

/// Rewrite edited hurtbox state and collision priority into the user's own source.
///
/// Value edits only, as everywhere on this path. The frame a state starts on is the block it
/// sits in rather than an argument, so a retime is reported; so is a change of target, since
/// `HIT_NODE` and `HIT_NO` are different macros taking different argument types and swapping
/// them is structure rather than value.
pub fn rewrite_hurtboxes(
    text: &str,
    label: &str,
    pristine: &(
        Vec<crate::data::HurtboxState>,
        Vec<crate::data::ColPriState>,
    ),
    edited: &(
        Vec<crate::data::HurtboxState>,
        Vec<crate::data::ColPriState>,
    ),
) -> Result<(String, SyncReport)> {
    use crate::data::HurtTarget;

    let sites = hurt_sites(text);
    let mut report = SyncReport::default();
    let mut edits = Vec::new();

    // Look the site up rather than pairing by position: a span list can be longer than the
    // statement list, because one looped call produces one span per iteration.
    let site_for = |site: usize, report: &mut SyncReport| -> Option<&MacroSite> {
        let found = sites.get(site);
        if found.is_none() {
            report.skipped.push(format!(
                "{label}: a hurtbox state has no matching call in the source — it was added in \
                 the editor, and source syncing only retunes existing calls"
            ));
        }
        found
    };

    let state_matches = crate::data::match_hurtbox_states(&pristine.0, &edited.0);
    for (edited_index, now) in edited.0.iter().enumerate() {
        let Some(source_index) = state_matches[edited_index] else {
            continue;
        };
        let before = &pristine.0[source_index];
        if before == now {
            continue;
        }
        let Some(site) = site_for(before.site, &mut report) else {
            continue;
        };
        if before.active_start != now.active_start {
            report.skipped.push(format!(
                "{label}: the `{}` on {} was retimed — its frame is the block it sits in, not an \
                 argument, so source syncing cannot move it",
                site.name,
                before.target.label()
            ));
        }
        // Retuning across a target change would write a bone hash into a group slot, which is
        // the cross-family corruption this codebase keeps finding the hard way.
        if std::mem::discriminant(&before.target) != std::mem::discriminant(&now.target) {
            report.skipped.push(format!(
                "{label}: {} became {} — that is a change from `{}` to `{}`, not a change of \
                 argument value",
                before.target.label(),
                now.target.label(),
                before.target.macro_name(),
                now.target.macro_name()
            ));
            continue;
        }
        match (&before.target, &now.target) {
            (HurtTarget::Bone(was), HurtTarget::Bone(is)) if was != is => {
                if let Some(span) = site.args.get(1) {
                    edits.extend(text_edit(
                        text,
                        span,
                        &crate::acmd::hash_arg(&is.to_ascii_lowercase()),
                    ));
                }
            }
            (HurtTarget::Group(was), HurtTarget::Group(is)) if was != is => {
                if let Some(span) = site.args.get(1) {
                    edits.extend(int_edit(text, span, *is));
                }
            }
            _ => {}
        }
        if before.status != now.status {
            // Slot 1 is `agent`, so the status is slot 2 for the two targeted macros and slot 1
            // for `WHOLE_HIT`, whose target is its name. Writing 2 unconditionally would put the
            // new status past the end of a `WHOLE_HIT`'s arguments and silently drop the edit.
            let slot = if now.target.takes_target_argument() {
                2
            } else {
                1
            };
            if let Some(span) = site.args.get(slot) {
                edits.extend(text_edit(
                    text,
                    span,
                    &crate::acmd::emit_status(&now.status),
                ));
            }
        }
    }

    let priority_matches = crate::data::match_col_pri_states(&pristine.1, &edited.1);
    for (edited_index, now) in edited.1.iter().enumerate() {
        let Some(source_index) = priority_matches[edited_index] else {
            continue;
        };
        let before = &pristine.1[source_index];
        if before == now {
            continue;
        }
        let Some(site) = site_for(before.site, &mut report) else {
            continue;
        };
        if before.active_start != now.active_start {
            report.skipped.push(format!(
                "{label}: a `COL_PRI` was retimed — its frame is the block it sits in, not an \
                 argument, so source syncing cannot move it"
            ));
        }
        if before.pri != now.pri {
            if let Some(span) = site.args.get(1) {
                edits.extend(int_edit(text, span, now.pri));
            }
        }
    }

    let value_edit_count = edits.len();
    let mut updated = apply(text, edits);
    // A parameter-range edit adds one `HIT_NO` at the range start and one at the restoring
    // frame. Those calls have no source site to retune, so place them into the nearest flat
    // frame block. Matching by target/status/start avoids treating an unchanged source span as
    // a new call when the evaluator's site ordinals shifted after an insertion. Branch/loop
    // placement is intentionally still conservative: the generated export remains authoritative
    // and the report explains why a hand-written source was left alone.
    // Only a schedule whose cardinality grew can contain newly authored calls.  A value edit
    // (changing a bone, group, or status) deliberately has no exact value match in `pristine`,
    // but it is still the same source statement and must be retuned in place rather than
    // duplicated.  The length guard keeps those ordinary edits out of the insertion path.  The
    // exact triple then identifies the additional states while allowing all unchanged source
    // states to remain byte-for-byte stable.
    for (edited_index, now) in edited.0.iter().enumerate() {
        if state_matches[edited_index].is_some() {
            continue;
        }
        if insert_hurt_line(&mut updated, now.active_start, &now.target, &now.status) {
            report.changed += 1;
        }
    }

    if pristine.0.len() != edited.0.len() || pristine.1.len() != edited.1.len() {
        report.skipped.push(format!(
            "{label}: the hurtbox schedule changed shape; newly authored ranges were inserted \
             where the source was structurally unambiguous, while the generated export remains \
             the complete representation"
        ));
    }

    report.changed += value_edit_count;
    Ok((updated, report))
}

/// The source-level `damage!` calls that set fighter-wide damage reaction.
///
/// `damage!` is a macro rather than a `macros::…` call, so it needs the named-call scanner. The
/// four arguments are `agent`, the MSC command, the mode, and the value. Keeping the command
/// check here is important: a different `damage!` command has the same argument count but a
/// different meaning and must not be rewritten as armor.
fn damage_no_reaction_sites(text: &str) -> Vec<MacroSite> {
    scan_named_sites(text, "damage!", 0..text.len())
        .into_iter()
        .filter(|site| {
            site.args.len() == 4
                && site.arg(text, 1).is_some_and(|command| {
                    command.trim().trim_start_matches('*') == "MA_MSC_DAMAGE_DAMAGE_NO_REACTION"
                })
        })
        .collect()
}

fn damage_no_reaction_tokens(condition: &crate::data::HurtboxCondition) -> (String, String) {
    match condition {
        crate::data::HurtboxCondition::Normal => {
            ("*DAMAGE_NO_REACTION_MODE_NORMAL".into(), "0".into())
        }
        crate::data::HurtboxCondition::SuperArmor => {
            ("*DAMAGE_NO_REACTION_MODE_ALWAYS".into(), "0".into())
        }
        crate::data::HurtboxCondition::ReactionValueArmor { threshold } => (
            "*DAMAGE_NO_REACTION_MODE_REACTION_VALUE".into(),
            threshold.to_string(),
        ),
        crate::data::HurtboxCondition::DamageBasedArmor { threshold } => (
            "*DAMAGE_NO_REACTION_MODE_DAMAGE_POWER".into(),
            threshold.to_string(),
        ),
        crate::data::HurtboxCondition::DamagePowerCount { threshold } => (
            "*DAMAGE_NO_REACTION_MODE_DAMAGE_POWER_COUNT".into(),
            threshold.to_string(),
        ),
        crate::data::HurtboxCondition::NoReactionMode => {
            ("*DAMAGE_NO_REACTION_MODE_NONE".into(), "0".into())
        }
        crate::data::HurtboxCondition::Unknown { mode, value } => {
            let raw = mode.trim();
            let mode =
                if raw.starts_with('*') || raw.parse::<i64>().is_ok() || raw.starts_with("0x") {
                    mode.clone()
                } else {
                    format!("*{mode}")
                };
            (mode, value.clone())
        }
    }
}

/// Rewrite the mode/value of existing fighter-wide damage-reaction calls and add new flat-frame
/// calls for newly authored armor ranges.
///
/// Frames remain structural source locations. A retimed existing call is therefore reported and
/// left in place, while a newly authored range is inserted into an existing execute block (or as
/// a small top-level frame block) using the same conservative helper as hurtbox-state insertion.
pub(crate) fn rewrite_hurtbox_conditions(
    text: &str,
    label: &str,
    pristine: &[crate::data::HurtboxConditionState],
    edited: &[crate::data::HurtboxConditionState],
) -> Result<(String, SyncReport)> {
    let sites = damage_no_reaction_sites(text);
    let mut report = SyncReport::default();
    let mut edits = Vec::new();

    let condition_matches = crate::data::match_hurtbox_conditions(pristine, edited);
    for (edited_index, now) in edited.iter().enumerate() {
        let Some(source_index) = condition_matches[edited_index] else {
            continue;
        };
        let before = &pristine[source_index];
        if before == now {
            continue;
        }
        let Some(site) = sites.get(before.site) else {
            report.skipped.push(format!(
                "{label}: a damage-reaction condition has no matching source call — source syncing only retunes existing calls"
            ));
            continue;
        };
        if before.active_start != now.active_start {
            report.skipped.push(format!(
                "{label}: {} was retimed from frame {} to {} — its frame is the source block, so source syncing cannot move it",
                before.condition.label(),
                before.active_start,
                now.active_start
            ));
        }
        if site.args.len() != 4 {
            report.skipped.push(format!(
                "{label}: damage-reaction call on frame {} has an unsupported argument shape",
                before.active_start
            ));
            continue;
        }
        let (mode, value) = damage_no_reaction_tokens(&now.condition);
        if let Some(span) = site.args.get(2) {
            if let Some(edit) = text_edit(text, span, &mode) {
                edits.push(edit);
            }
        }
        if let Some(span) = site.args.get(3) {
            if let Some(edit) = text_edit(text, span, &value) {
                edits.push(edit);
            }
        }
    }

    // The evaluator exposes one state per authored call, including normal restoration points.
    // Site identity distinguishes a value edit to an existing call from a newly authored range;
    // matching condition text here would mistake a changed super-armor mode for an insertion.
    let mut replacement_spans = Vec::new();
    for edit in &edits {
        if !replacement_spans.iter().any(|span| span == &edit.span) {
            replacement_spans.push(edit.span.clone());
        }
    }
    let value_edit_count = replacement_spans.len();
    let mut updated = apply(text, edits);
    for (edited_index, now) in edited.iter().enumerate() {
        if condition_matches[edited_index].is_some() {
            continue;
        }
        if insert_damage_no_reaction_line(&mut updated, now.active_start, &now.condition) {
            report.changed += 1;
        }
    }

    if pristine.len() != edited.len() {
        report.skipped.push(format!(
            "{label}: the damage-reaction schedule changed shape; newly authored ranges were inserted where the source was structurally unambiguous"
        ));
    }
    // Added calls were counted above. Existing value replacements are counted by distinct source
    // spans so a looped event cannot report the same source line twice.
    report.changed += value_edit_count;
    Ok((updated, report))
}

/// Sync edited fighter-wide damage-reaction conditions into the project's `game_` source.
pub(crate) fn sync_hurtbox_conditions(
    index: &SourceIndex,
    fighter: &str,
    move_name: &str,
    pristine: &[crate::data::HurtboxConditionState],
    edited: &[crate::data::HurtboxConditionState],
) -> Result<SyncReport> {
    let script_name = crate::acmd::acmd_script_name("game", move_name);
    sync_script(index, fighter, &script_name, |body| {
        rewrite_hurtbox_conditions(body, &format!("{fighter}/{move_name}"), pristine, edited)
    })
}

// ── Attack modifier write-back ───────────────────────────────────────────────

/// The post-hoc tuning commands, with the argument count each one has after `agent`.
///
/// Both take `(id, value)`, so unlike [`HURT_COMMANDS`] there is no per-macro asymmetry to get
/// wrong here. The arity is still part of identifying the call, for the same reason: the parser
/// leaves a wrong-arity call as `Raw`, and counting it anyway would number every later site one
/// too high and retune a different call.
///
/// `ATK_HIT_ABS` and `ATK_LERP_RATIO` are deliberately absent — they take no id, so they are not
/// this family and have no site here.
const ATTACK_MOD_COMMANDS: &[(&str, usize)] = &[("ATK_POWER", 2), ("ATK_SET_SHIELD_SETOFF_MUL", 2)];

/// The attack-modifier calls in `text`, in document order — index `n` is site `n`.
///
/// Its own scan and its own numbering, not a share of [`hurt_sites`]: the two families are
/// counted separately in the parsed script too, so that adding an `ATK_POWER` to a move cannot
/// shift the site of a `HIT_NODE` that comes after it.
fn attack_mod_sites(text: &str) -> Vec<MacroSite> {
    scan_macro_sites(text, 0..text.len())
        .into_iter()
        .filter(|site| {
            ATTACK_MOD_COMMANDS
                .iter()
                .any(|(name, arity)| site.name == *name && site.args.len() == arity + 1)
        })
        .collect()
}

/// Rewrite edited post-hoc hitbox modifiers into the user's own source.
///
/// Value edits only, as everywhere on this path. A retime is reported rather than written, since
/// the frame is the block the call sits in and not an argument; so is a change of *kind*, which
/// is a different macro rather than a different value.
pub fn rewrite_attack_mods(
    text: &str,
    label: &str,
    pristine: &[crate::data::AttackModState],
    edited: &[crate::data::AttackModState],
) -> Result<(String, SyncReport)> {
    let sites = attack_mod_sites(text);
    let mut report = SyncReport::default();
    let mut edits = Vec::new();

    for (before, now) in pristine.iter().zip(edited.iter()) {
        if before == now {
            continue;
        }
        let Some(site) = sites.get(now.site) else {
            report.skipped.push(format!(
                "{label}: a `{}` has no matching call in the source — it was added in the \
                 editor, and source syncing only retunes existing calls",
                now.kind.macro_name()
            ));
            continue;
        };
        if before.frame != now.frame {
            report.skipped.push(format!(
                "{label}: the `{}` on hitbox {} was retimed — its frame is the block it sits in, \
                 not an argument, so source syncing cannot move it",
                before.kind.macro_name(),
                before.id
            ));
        }
        if before.kind != now.kind {
            report.skipped.push(format!(
                "{label}: a `{}` became a `{}` — that is a different macro, not a change of \
                 argument value",
                before.kind.macro_name(),
                now.kind.macro_name()
            ));
            continue;
        }
        // Slot 0 is `agent`, so the id is 1 and the value is 2 — the order `macros.rs` declares
        // and the order `ATK_POWER`'s two vanilla calls confirm by varying the id alone.
        if before.id != now.id {
            if let Some(span) = site.args.get(1) {
                edits.extend(int_edit(text, span, now.id));
            }
        }
        if before.value != now.value {
            if let Some(span) = site.args.get(2) {
                // `to_f32_edit`, not `float_edit`: these slots are `ToF32`-generic and every
                // vanilla call writes a bare integer, so a value of 7 must stay `7` and not
                // become `7.0` — the same rule `crate::acmd::attack_mod_num` follows on export.
                edits.extend(to_f32_edit(text, span, now.value));
            }
        }
    }

    if pristine.len() != edited.len() {
        report.skipped.push(format!(
            "{label}: hitbox modifiers were added or removed — source syncing only retunes \
             existing calls, so use an export to land that"
        ));
    }

    report.changed = edits.len();
    Ok((apply(text, edits), report))
}

// ── Playback rate write-back ─────────────────────────────────────────────────

/// The `FT_MOTION_RATE` calls in `text`, in document order — index `n` is site `n`.
///
/// **Name equality, not a prefix.** `FT_MOTION_RATE_RANGE` starts with this macro's entire name
/// and takes three arguments after `agent`; counting it here would number every later site one
/// too high and write a rate into an unrelated call. The arity filter says the same thing twice
/// on purpose, because both of these have gone wrong before in this codebase.
fn motion_rate_sites(text: &str) -> Vec<MacroSite> {
    scan_macro_sites(text, 0..text.len())
        .into_iter()
        .filter(|site| site.name == "FT_MOTION_RATE" && site.args.len() == 2)
        .collect()
}

/// Rewrite edited playback rates into the user's own source.
///
/// `edited` is the script's rate calls in script order, as
/// [`AcmdScript::motion_rate_sites`](crate::data::AcmdScript::motion_rate_sites) gives them.
/// **The source is its own pristine copy** — the value in the file is what the game plays — so
/// unlike the other families here this takes no separate baseline. That is possible because a
/// rate edit changes the argument and nothing else: it cannot renumber, retime, or change which
/// macro is called.
///
/// **The counts must agree, and this refuses rather than guessing when they do not.** The parser
/// only models a rate call at the function's top level, so one written inside a runtime branch is
/// kept as a raw line and never reaches `edited` — while `motion_rate_sites` above would still
/// count it. That mismatch is exactly the site-ordinal drift that silently retargets a later
/// edit, so it is reported instead.
pub fn rewrite_motion_rates(
    text: &str,
    label: &str,
    edited: &[(usize, f32, f32)],
) -> Result<(String, SyncReport)> {
    let sites = motion_rate_sites(text);
    let mut report = SyncReport::default();

    if sites.len() != edited.len() {
        report.skipped.push(format!(
            "{label}: the source has {} `FT_MOTION_RATE` call(s) and the editor models {} — one \
             of them is inside a branch, which this cannot address by position",
            sites.len(),
            edited.len()
        ));
        return Ok((text.to_string(), report));
    }

    let mut edits = Vec::new();
    for (site, (_, _, rate)) in sites.iter().zip(edited.iter()) {
        // Slot 0 is `agent`, so the rate is slot 1 — the order `macros.rs` declares.
        if let Some(span) = site.args.get(1) {
            // `float_edit`, not `to_f32_edit`: every corpus call writes a decimal (`0.5`, `1.0`),
            // and the argument is `ToF32`-generic, so keeping the decimal form matches both the
            // source and what `acmd::num` emits.
            edits.extend(float_edit(text, span, *rate));
        }
    }

    report.changed = edits.len();
    Ok((apply(text, edits), report))
}

/// Sync edited playback rates for one move back into the project source on disk.
pub fn sync_motion_rates(
    index: &SourceIndex,
    fighter: &str,
    move_name: &str,
    edited: &[(usize, f32, f32)],
) -> Result<SyncReport> {
    let script_name = crate::acmd::acmd_script_name("game", move_name);
    sync_script(index, fighter, &script_name, |body| {
        rewrite_motion_rates(body, &format!("{fighter}/{move_name}"), edited)
    })
}

// ── Article call write-back ─────────────────────────────────────────────────

/// The module paths an article call is written with, as [`crate::data::ArticleCall::parse`]
/// reads them.
const ARTICLE_MODULE: &str = "ArticleModule::";
const ARTICLE_LUA_BIND: &str = "smash::app::lua_bind::";

/// Every `ArticleModule::f(..)` call in `text` written as a whole statement, in document order:
/// `[let x = ][smash::app::lua_bind::]ArticleModule::f(args);`.
///
/// Located in the file's own text, never rebuilt from the editor's model of a call (a timeline
/// call carries the timeline's spelling, not the file's). A call inside an expression
/// (`if ArticleModule::is_exist(..)`, an argument of another call) is not a statement and is not
/// counted: the parser keeps those as raw lines, so the walk never reports them either.
pub(crate) fn article_call_sites(text: &str) -> Vec<MacroSite> {
    let bytes = text.as_bytes();
    let mut sites = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if let Some(next) = skip_rust_literal_or_comment(text, i) {
            i = next;
            continue;
        }
        if !text.is_char_boundary(i) || !text[i..].starts_with(ARTICLE_MODULE) {
            i += 1;
            continue;
        }
        let start = if text[..i].ends_with(ARTICLE_LUA_BIND) {
            i - ARTICLE_LUA_BIND.len()
        } else {
            i
        };
        let name_start = i + ARTICLE_MODULE.len();
        // Not a longer or differently qualified name that merely ends in `ArticleModule::`.
        if text[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':' || c == '.')
        {
            i = name_start;
            continue;
        }
        let mut name_end = name_start;
        while name_end < text.len()
            && (bytes[name_end].is_ascii_alphanumeric() || bytes[name_end] == b'_')
        {
            name_end += 1;
        }
        if name_end == name_start || bytes.get(name_end) != Some(&b'(') {
            i = name_end;
            continue;
        }
        let Some((args, close)) = split_call_args(text, name_end + 1, text.len()) else {
            i = name_end + 1;
            continue;
        };
        i = close + 1;
        if article_statement_semicolon(text, start, close).is_some() {
            sites.push(MacroSite {
                name: text[name_start..name_end].to_string(),
                span: start..close + 1,
                args,
            });
        }
    }
    sites
}

/// The `;` ending the statement a call from `start` to its `)` at `close` makes, if the call is
/// a whole statement: preceded by the start of a block or statement (`{`, `;`, `}`), optionally
/// through a `let` binding on the same line, and followed by `;`.
fn article_statement_semicolon(text: &str, start: usize, close: usize) -> Option<usize> {
    let after = &text[close + 1..];
    let semicolon = close + 1 + (after.len() - after.trim_start().len());
    if text.as_bytes().get(semicolon) != Some(&b';') {
        return None;
    }
    let line_start = text[..start].rfind('\n').map_or(0, |at| at + 1);
    let prefix = text[line_start..start].trim();
    let before = if prefix.is_empty() {
        &text[..line_start]
    } else if let Some(binding) = prefix.strip_prefix("let ") {
        // `let id = ` and nothing else: no operator before the `=`.
        let pattern = binding.strip_suffix('=')?.trim_end();
        if pattern.is_empty()
            || pattern.ends_with(['=', '!', '<', '>', '+', '-', '*', '/', '%', '&', '|', '^'])
            || pattern.contains(['(', ';', '"', '{', '}'])
        {
            return None;
        }
        let at = text[line_start..start].find("let ")? + line_start;
        &text[..at]
    } else {
        &text[..start]
    };
    matches!(last_code_char(before), None | Some('{' | ';' | '}')).then_some(semicolon)
}

/// The last character of `text` that is code: not whitespace and not inside a comment.
fn last_code_char(text: &str) -> Option<char> {
    let mut last = None;
    let mut i = 0;
    while i < text.len() {
        if let Some(next) = skip_rust_literal_or_comment(text, i) {
            if !(text[i..].starts_with("//") || text[i..].starts_with("/*")) {
                last = text[..next.min(text.len())].chars().next_back();
            }
            i = next;
            continue;
        }
        let Some(c) = text[i..].chars().next() else {
            break;
        };
        if !c.is_whitespace() {
            last = Some(c);
        }
        i += c.len_utf8();
    }
    last
}

/// The frame block `site` sits in, as [`source_frame_block`] finds it, when its `frame(..)` line
/// comes before the `is_excute` block holding the site and on a line of its own. (A `frame` inside
/// the block, or on the block's own line, is not a frame block this can retime.)
fn article_frame_block(text: &str, site: &MacroSite) -> Option<EffectSourceBlock> {
    let (open, _) = enclosing_effect_execute(text, site.span.start)?;
    let frame = scan_named_sites(text, "frame", 0..text.len())
        .into_iter()
        .rev()
        .find(|frame| frame.span.end <= site.span.start)?;
    let frame_line = line_range_at(text, frame.span.start)?;
    let execute_line = line_range_at(text, open)?;
    if execute_line.start < frame_line.end {
        return None;
    }
    source_frame_block(text, site)
}

/// Why the call at `site` is not `call` at `frame`, if it is not. The writer pairs by position
/// with a list read from this same text, so every field must agree: function, argument count,
/// article, every argument (the target by what it names, since a timeline call carries the
/// timeline's spelling of it) and, where the source states one, the frame.
fn article_pairing_mismatch(
    text: &str,
    site: &MacroSite,
    frame: u32,
    call: &crate::data::ArticleCall,
) -> Option<String> {
    let squash = |s: &str| s.split_whitespace().collect::<String>();
    if site.name != call.op.name() {
        return Some(format!("the source calls `{}` where the list has `{}`", site.name, call.op.name()));
    }
    if site.args.len() != call.args.len() + 2 {
        return Some(format!(
            "the source's `{}` has {} argument(s) where the list has {}",
            site.name,
            site.args.len(),
            call.args.len() + 2
        ));
    }
    if squash(&text[site.args[1].clone()]) != squash(&call.article) {
        return Some(format!(
            "the source's `{}` acts on `{}` where the list has `{}`",
            site.name,
            text[site.args[1].clone()].trim(),
            call.article
        ));
    }
    let target_slot = call.op.target_index();
    for (slot, expected) in call.args.iter().enumerate() {
        let written = &text[site.args[slot + 2].clone()];
        let agrees = if Some(slot) == target_slot {
            crate::data::ArticleTarget::read(written) == call.target
        } else {
            squash(written) == squash(expected)
        };
        if !agrees {
            return Some(format!(
                "the source's `{}` passes `{}` where the list has `{expected}`",
                site.name,
                written.trim()
            ));
        }
    }
    if let Some(block) = article_frame_block(text, site) {
        if block.frame != frame {
            return Some(format!(
                "the source's `{}` is under frame {} where the list has it at frame {frame}",
                site.name, block.frame
            ));
        }
    }
    None
}

/// Rewrite edited article calls into the user's own source.
///
/// `pristine` is the calls `text` holds, as [`AcmdScript::to_article_calls`]
/// (crate::data::AcmdScript::to_article_calls) reads them from it, and `edited` is the same list
/// with the user's edits, as (script frame, call). They are paired with the file's own call
/// statements **by position**, the way [`rewrite_motion_rates`] pairs rates, and every pair must
/// agree exactly ([`article_pairing_mismatch`]); otherwise nothing is written and the reason is
/// reported. The editor's own lists go through [`rewrite_editor_article_calls`], which puts them
/// in this form.
///
/// - An argument edit replaces only that argument's bytes, so the receiver, any `let` binding,
///   the module path and the spacing stay as written. A changed target keeps the file's own
///   spelling of it (`ArticleOperationTarget(..)` or bare, symbolic or numeric).
/// - A frame change moves the statement to the new frame's block, creating the block when the
///   function has none, as effect blocks move. See [`move_call_to_frame`].
/// - Changing which `ArticleModule` function is called, or adding or removing arguments or calls,
///   is structural and reported.
pub fn rewrite_article_calls(
    text: &str,
    label: &str,
    pristine: &[(u32, crate::data::ArticleCall)],
    edited: &[(u32, crate::data::ArticleCall)],
) -> Result<(String, SyncReport)> {
    let mut report = SyncReport::default();
    let same = article_call_unchanged;
    if pristine.len() == edited.len() && pristine.iter().zip(edited).all(|(a, b)| same(a, b)) {
        return Ok((text.to_string(), report));
    }
    if pristine.len() != edited.len() {
        report.skipped.push(format!(
            "{label}: article calls were added or removed — source syncing only retunes existing \
             calls, so use an export to land that"
        ));
        return Ok((text.to_string(), report));
    }
    let sites = article_call_sites(text);
    if sites.len() != pristine.len() {
        report.skipped.push(format!(
            "{label}: the source has {} `ArticleModule` call(s) and the editor models {} — one of \
             them is inside a branch, a loop or an expression, which this cannot address by \
             position",
            sites.len(),
            pristine.len()
        ));
        return Ok((text.to_string(), report));
    }
    for (index, (site, (frame, call))) in sites.iter().zip(pristine).enumerate() {
        if let Some(reason) = article_pairing_mismatch(text, site, *frame, call) {
            report.skipped.push(format!(
                "{label}: article call #{} is not what the source holds there ({reason}), so no \
                 article call was changed; reload the move",
                index + 1
            ));
            return Ok((text.to_string(), report));
        }
    }
    let assigned: Vec<usize> = (0..sites.len()).collect();

    let mut edits = Vec::new();
    let mut moves = Vec::new();
    for (site, &index) in sites.iter().zip(&assigned) {
        let (was, now) = (&pristine[index], &edited[index]);
        if same(was, now) {
            continue;
        }
        let ((from, before), (to, after)) = (was, now);
        let number = index + 1;
        if before.op != after.op {
            report.skipped.push(format!(
                "{label}: article call #{number} changes which function it calls (`{}` to `{}`) — \
                 source syncing only retunes existing calls, so use an export to land that",
                before.op.name(),
                after.op.name()
            ));
            continue;
        }
        if before.args.len() != after.args.len() {
            report.skipped.push(format!(
                "{label}: article call #{number} (`{}`) had arguments added or removed — source \
                 syncing only retunes existing arguments",
                site.name
            ));
            continue;
        }
        let mut replace = |span: &Range<usize>, value: String| {
            if text[span.clone()].trim() != value.trim() {
                edits.push(Replacement { span: span.clone(), value });
            }
        };
        if before.article != after.article {
            replace(&site.args[1], after.article.clone());
        }
        let target_slot = before.op.target_index();
        let squash = |s: &str| s.split_whitespace().collect::<String>();
        for (slot, (was, now)) in before.args.iter().zip(&after.args).enumerate() {
            let span = &site.args[slot + 2];
            if Some(slot) == target_slot && after.target != crate::data::ArticleTarget::Unspecified {
                // By what it names only: the text in `args` may still be the spelling from before
                // an edit that set `target`. Written in the file's own spelling, not the editor's.
                if before.target != after.target {
                    replace(span, after.target.respell(&text[span.clone()]));
                }
            } else if squash(was) != squash(now) {
                replace(span, now.clone());
            }
        }
        if from != to {
            moves.push((index, *from, *to));
        }
    }

    report.changed = edits.len();
    let mut out = apply(text, edits);
    // Which editor call each statement in `out` is, in document order: a move can carry a call
    // past others, and every later move must still find its own call.
    let mut order: Vec<usize> = assigned;
    for (index, from, to) in moves {
        let number = index + 1;
        let current = article_call_sites(&out);
        let Some(site) = order
            .iter()
            .position(|&call| call == index)
            .and_then(|position| current.get(position))
        else {
            report.skipped.push(format!(
                "{label}: article call #{number} could not be found again after an earlier move"
            ));
            break;
        };
        if article_frame_block(&out, site).is_some_and(|block| block.frame == to) {
            continue; // Already there: a sync ran before with this edit.
        }
        match move_call_to_frame(&out, site, from, to) {
            Ok((moved, at)) => {
                let after = article_call_sites(&moved);
                let Some(position) = after.iter().position(|site| site.span.start == at) else {
                    report.skipped.push(format!(
                        "{label}: article call #{number} moved, but could not be found again, so no \
                         later article call was moved"
                    ));
                    out = moved;
                    report.changed += 1;
                    break;
                };
                order.retain(|&call| call != index);
                order.insert(position, index);
                out = moved;
                report.changed += 1;
            }
            Err(reason) => report.skipped.push(format!(
                "{label}: `{}` (article call #{number}) could not move from frame {from} to frame \
                 {to}: {reason}",
                site.name
            )),
        }
    }
    Ok((out, report))
}

/// The editor's edited article calls in the order, and against the values, of `source`: the
/// calls `text` holds now, read with the same parser (`to_article_calls`). This is the per-write
/// baseline hitboxes and effects use. An earlier sync may have moved a call past another,
/// renamed its article, or retimed it, so the file's list and the editor's original list
/// (`pristine`, edited into `edited`) no longer line up by position.
///
/// A source call can be editor call `i` only when they have the same function and argument count
/// and every field (frame, article, each argument, the target by what it names) is either as call
/// `i` was, as it is edited, or — only for a field the user changed on that call — some value in
/// between that an earlier sync wrote. Among those, a call that agrees on every field is paired
/// first; then each remaining source call takes its one best match. Several equal matches are
/// accepted only when they are the same call, edited the same way. Anything else is refused with
/// the reason, and nothing is guessed by position.
pub fn article_calls_in_source_order(
    source: &[(u32, crate::data::ArticleCall)],
    pristine: &[(u32, crate::data::ArticleCall)],
    edited: &[(u32, crate::data::ArticleCall)],
) -> std::result::Result<Vec<(u32, crate::data::ArticleCall)>, String> {
    if pristine.len() != edited.len() {
        return Err(
            "article calls were added or removed — source syncing only retunes existing calls, so \
             use an export to land that"
                .into(),
        );
    }
    if source.len() != pristine.len() {
        return Err(format!(
            "the source has {} article call(s) where the editor has {}, so they cannot be matched \
             up; reload the move",
            source.len(),
            pristine.len()
        ));
    }
    // The file holds exactly the calls the editor loaded, in their order: call `i` is the `i`th,
    // even where several are the same call (Cuphead's four identical bullets in one frame).
    if source.iter().zip(pristine).all(|(held, loaded)| article_call_unchanged(held, loaded)) {
        return Ok(edited.to_vec());
    }
    let was: Vec<Vec<String>> = pristine.iter().map(article_call_fields).collect();
    let then: Vec<Vec<String>> = edited.iter().map(article_call_fields).collect();
    // `None`: this source call cannot be editor call `index`. Otherwise how many fields agree,
    // and whether all of them do.
    let score = |(_, call): &(u32, crate::data::ArticleCall), fields: &[String], index: usize| {
        let (before, after) = (&pristine[index].1, &edited[index].1);
        if (call.op != before.op && call.op != after.op) || call.args.len() != before.args.len() {
            return None;
        }
        let mut agree = 0;
        for (field, now) in fields.iter().enumerate() {
            let old = &was[index][field];
            let new = then[index].get(field).unwrap_or(old);
            if now == old || now == new {
                agree += 1;
            } else if old == new {
                // A value neither side has, on a field the user did not change: not this call.
                return None;
            }
        }
        Some((agree, agree == fields.len()))
    };
    let interchangeable = |a: usize, b: usize| {
        article_call_unchanged(&pristine[a], &pristine[b]) && article_call_unchanged(&edited[a], &edited[b])
    };
    let fields: Vec<Vec<String>> = source.iter().map(article_call_fields).collect();

    let mut assigned: Vec<Option<usize>> = vec![None; source.len()];
    let mut used = vec![false; pristine.len()];
    while assigned.iter().any(Option::is_none) {
        let mut progress = false;
        // Calls that agree on everything settle first; only then the closest match.
        for full_only in [true, false] {
            for (position, call) in source.iter().enumerate() {
                if assigned[position].is_some() {
                    continue;
                }
                let scored: Vec<(usize, usize, bool)> = (0..pristine.len())
                    .filter(|&index| !used[index])
                    .filter_map(|index| score(call, &fields[position], index).map(|(agree, full)| (index, agree, full)))
                    .filter(|(_, _, full)| *full || !full_only)
                    .collect();
                if scored.is_empty() {
                    if full_only {
                        continue;
                    }
                    return Err(format!(
                        "the source's `{}` call on {} at frame {} matches none of the editor's article \
                         calls as they were or as edited, so none was changed; reload the move",
                        call.1.op.name(),
                        call.1.article,
                        call.0
                    ));
                }
                let best = scored.iter().map(|(_, agree, _)| *agree).max().unwrap_or(0);
                let top: Vec<usize> = scored
                    .iter()
                    .filter(|(_, agree, _)| *agree == best)
                    .map(|(index, _, _)| *index)
                    .collect();
                if top.iter().all(|&index| interchangeable(top[0], index)) {
                    assigned[position] = Some(top[0]);
                    used[top[0]] = true;
                    progress = true;
                }
            }
            if progress {
                break;
            }
        }
        if !progress {
            let position = assigned.iter().position(Option::is_none).unwrap_or(0);
            let (frame, call) = &source[position];
            return Err(format!(
                "the source's `{}` call on {} at frame {frame} could be more than one of the \
                 editor's article calls, so none was changed; reload the move, or edit one call at \
                 a time and sync in between",
                call.op.name(),
                call.article
            ));
        }
    }
    Ok(assigned
        .into_iter()
        .map(|index| edited[index.expect("every source call was assigned")].clone())
        .collect())
}

/// [`rewrite_article_calls`] for the editor's lists: the baseline is what `text` holds now (see
/// [`article_calls_in_source_order`]), so a sync after an earlier sync of the same move still
/// pairs each call with its own statement. When the lists cannot be matched, the reason is
/// reported unless the file already holds exactly the edited calls.
pub fn rewrite_editor_article_calls(
    text: &str,
    label: &str,
    pristine: &[(u32, crate::data::ArticleCall)],
    edited: &[(u32, crate::data::ArticleCall)],
) -> Result<(String, SyncReport)> {
    let source = crate::acmd::parse_acmd_script(text).to_article_calls();
    match article_calls_in_source_order(&source, pristine, edited) {
        Ok(ordered) => rewrite_article_calls(text, label, &source, &ordered),
        Err(reason) => {
            let mut report = SyncReport::default();
            // Nothing to do only when the FILE already holds every edited call, in any order.
            let mut left: Vec<&(u32, crate::data::ArticleCall)> = source.iter().collect();
            let settled = source.len() == edited.len()
                && edited.iter().all(|call| {
                    left.iter()
                        .position(|held| article_call_unchanged(held, call))
                        .map(|at| left.remove(at))
                        .is_some()
                });
            if !settled {
                report.skipped.push(format!("{label}: {reason}"));
            }
            Ok((text.to_string(), report))
        }
    }
}

/// What an article call does, field by field, as comparable text: its frame, its article, then
/// every argument with whitespace removed. A known target is compared by what it names, since a
/// timeline call keeps the timeline's spelling of it and an edit may set only `target`.
fn article_call_fields((frame, call): &(u32, crate::data::ArticleCall)) -> Vec<String> {
    let squash = |s: &str| s.split_whitespace().collect::<String>();
    let target_slot = call.op.target_index();
    let mut fields = vec![frame.to_string(), squash(&call.article)];
    for (slot, arg) in call.args.iter().enumerate() {
        fields.push(if Some(slot) == target_slot && call.target != crate::data::ArticleTarget::Unspecified {
            format!("target {:?}", call.target)
        } else {
            squash(arg)
        });
    }
    fields
}

/// Whether two calls do the same thing: `source`, `edited` and `origin` only say how one was read.
fn article_call_unchanged(a: &(u32, crate::data::ArticleCall), b: &(u32, crate::data::ArticleCall)) -> bool {
    a.1.op == b.1.op && article_call_fields(a) == article_call_fields(b)
}

/// Sync edited article calls for one move back into the project source on disk. `pristine` and
/// `edited` are the editor's lists; the baseline is re-read from the file on every sync.
pub fn sync_article_calls(
    index: &SourceIndex,
    fighter: &str,
    move_name: &str,
    pristine: &[(u32, crate::data::ArticleCall)],
    edited: &[(u32, crate::data::ArticleCall)],
) -> Result<SyncReport> {
    let script_name = crate::acmd::acmd_script_name("game", move_name);
    sync_script(index, fighter, &script_name, |body| {
        rewrite_editor_article_calls(body, &format!("{fighter}/{move_name}"), pristine, edited)
    })
}

/// Why frame blocks in `text` do not say when a line runs, if they do not: a `wait` makes the
/// time a block runs at depend on what came before it, and a loop runs its blocks more than once.
fn frame_order_unreliable(text: &str) -> Option<&'static str> {
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < text.len() {
        if let Some(next) = skip_rust_literal_or_comment(text, i) {
            i = next;
            continue;
        }
        let word_start = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
        if word_start {
            // `wait(..)`, `macros::wait(..)` and the `wait_loop_*` family all hold the script.
            let rest = &text[i..];
            let wait_call = rest.strip_prefix("wait").is_some_and(|after| {
                after.trim_start().starts_with('(') || after.starts_with("_loop")
            });
            if wait_call {
                return Some("the script uses `wait`, so a frame block's place does not say when it runs");
            }
            for keyword in ["for", "while", "loop"] {
                let end = i + keyword.len();
                if text[i..].starts_with(keyword)
                    && bytes.get(end).is_some_and(|b| b.is_ascii_whitespace() || *b == b'{')
                {
                    return Some("the script has a loop, so its frame blocks can run more than once");
                }
            }
        }
        i += 1;
        while i < text.len() && !text.is_char_boundary(i) {
            i += 1;
        }
    }
    None
}

/// The brace depth of `at` inside `body` (0 directly in it), ignoring literals and comments.
fn brace_depth(text: &str, body: Range<usize>, at: usize) -> usize {
    let mut depth = 0usize;
    let mut i = body.start;
    while i < at {
        if let Some(next) = skip_rust_literal_or_comment(text, i) {
            i = next;
            continue;
        }
        match text.as_bytes()[i] {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
        i += 1;
    }
    depth
}

/// The frame argument `old` respelled as `frame`: `10.0` gives `14.0`, `10` gives `14`.
fn frame_arg_like(old: &str, frame: u32) -> String {
    crate::project_consts::format_literal_like(old, f64::from(frame))
        .unwrap_or_else(|| format_float(frame as f32))
}

/// Move the statement calling `site` from its frame block (at frame `from`) to frame `to`.
///
/// This works for any call `site` locates, not only effects. The statement must be a whole line
/// (or lines) directly inside a `frame(..); if is_excute(..) { .. }` block, the shape effect
/// timing moves accept. Then:
///
/// - If it is the block's only statement (comments aside) and no block exists at `to`, the whole
///   block, comments included, is retimed and placed in frame order, exactly as an effect block
///   moves.
/// - If a block already exists at `to`, the statement joins the end of it, and a block the
///   statement leaves with nothing in it at all is removed; one left holding comments stays.
/// - Otherwise a new block is made at `to` from the frame line, `is_excute` header and closing
///   line of the block it left, so it is written the way the user writes blocks.
///
/// Every byte moved keeps its own line endings and indentation. Returns the new text and where
/// the moved call now starts, or why it cannot be moved.
fn move_call_to_frame(text: &str, site: &MacroSite, from: u32, to: u32) -> Result<(String, usize), String> {
    if let Some(reason) = frame_order_unreliable(text) {
        return Err(reason.into());
    }
    let block = article_frame_block(text, site).ok_or(
        "it is not directly inside a `frame(..)` statement's `is_excute` block",
    )?;
    if block.frame != from {
        return Err(format!("its block is at frame {}, not {from}", block.frame));
    }
    if brace_depth(text, block.execute_body.clone(), site.span.start) != 0 {
        return Err("it is inside a condition or block within its frame block".into());
    }
    let line_of = |at: usize| line_range_at(text, at).ok_or("its line could not be found");
    let start_line = line_of(site.span.start)?;
    let prefix = text[start_line.start..site.span.start].trim();
    if !(prefix.is_empty() || prefix.starts_with("let ")) {
        return Err("another statement shares its line".into());
    }
    let semicolon = article_statement_semicolon(text, site.span.start, site.span.end - 1)
        .or_else(|| {
            let after = &text[site.span.end..];
            let at = site.span.end + (after.len() - after.trim_start().len());
            (text.as_bytes().get(at) == Some(&b';')).then_some(at)
        })
        .ok_or("it is not a whole statement")?;
    let end_line = line_of(semicolon)?;
    let tail = text[semicolon + 1..end_line.end].trim();
    if !(tail.is_empty() || tail.starts_with("//")) {
        return Err("another statement shares its line".into());
    }
    let statement = start_line.start..end_line.end;
    if statement.start < block.execute_body.start || statement.end > block.execute_body.end {
        return Err("its block opens or closes on the same line as it".into());
    }
    let statement_text = &text[statement.clone()];
    let call_offset = site.span.start - statement.start;
    // What else the block holds: nothing at all, or only comments (which move with a block that
    // moves whole, as an effect block's do, and stay put otherwise).
    let rest = format!(
        "{}{}",
        &text[block.execute_body.start..statement.start],
        &text[statement.end..block.execute_body.end]
    );
    let empty = rest.trim().is_empty();
    let alone = rest.lines().all(|line| {
        let line = line.trim();
        line.is_empty()
            || line.starts_with("//")
            || line.starts_with("/*")
            || line.starts_with('*')
            || line.ends_with("*/")
    });

    // Where a block closes: the start of the line holding its `}`, which must hold nothing else.
    let closing_line = |close: usize| -> Result<Range<usize>, String> {
        let line = line_of(close)?;
        if !text[line.start..close].trim().is_empty() {
            return Err(format!(
                "a block at frame {to} closes on the same line as its last statement"
            ));
        }
        Ok(line)
    };
    let target = source_frame_blocks(text)
        .into_iter()
        .find(|candidate| candidate.frame == to && candidate.range != block.range);

    // (removed range, insertion point, inserted text, call's offset in the inserted text)
    let (removed, anchor, inserted, offset) = match &target {
        Some(target) => {
            let anchor = closing_line(target.execute_body.end)?.start;
            let removed = if empty { block.range.clone() } else { statement.clone() };
            (removed, anchor, statement_text.to_string(), call_offset)
        }
        None => {
            let frame_arg = block.frame_arg.clone();
            let new_arg = frame_arg_like(&text[frame_arg.clone()], to);
            let (removed, chunk, offset) = if alone {
                let relative = frame_arg.start - block.range.start..frame_arg.end - block.range.start;
                let mut chunk = text[block.range.clone()].to_string();
                let growth = new_arg.len() as isize - relative.len() as isize;
                chunk.replace_range(relative, &new_arg);
                let offset = (site.span.start - block.range.start) as isize + growth;
                (block.range.clone(), chunk, offset as usize)
            } else {
                // `{` ends its header line and `}` starts its closing line, or the block cannot
                // be copied without the statements it holds.
                let header_end = line_of(block.execute_body.start - 1)?.end;
                if !text[block.execute_body.start..header_end].trim().is_empty() {
                    return Err("its block's `is_excute` line also holds a statement".into());
                }
                let closer = closing_line(block.execute_body.end)
                    .map_err(|_| "its block closes on the same line as a statement".to_string())?;
                let mut header = text[block.range.start..header_end].to_string();
                header.replace_range(
                    frame_arg.start - block.range.start..frame_arg.end - block.range.start,
                    &new_arg,
                );
                let offset = header.len() + call_offset;
                let chunk = format!("{header}{statement_text}{}", &text[closer.start..block.range.end]);
                (statement.clone(), chunk, offset)
            };
            // A block the call leaves behind keeps its frame line, which still orders the rest.
            let moving = if alone { block.range.clone() } else { statement.clone() };
            (removed, frame_insertion_point(text, to, &moving)?, chunk, offset)
        }
    };
    if removed.start < anchor && anchor < removed.end {
        return Err("its new place is inside the lines it leaves".into());
    }
    let mut out = String::with_capacity(text.len() + inserted.len());
    let at = if anchor <= removed.start {
        out.push_str(&text[..anchor]);
        let at = out.len() + offset;
        out.push_str(&inserted);
        out.push_str(&text[anchor..removed.start]);
        out.push_str(&text[removed.end..]);
        at
    } else {
        out.push_str(&text[..removed.start]);
        out.push_str(&text[removed.end..anchor]);
        let at = out.len() + offset;
        out.push_str(&inserted);
        out.push_str(&text[anchor..]);
        at
    };
    Ok((out, at))
}

/// Where a new block for `frame` goes: before the first `frame(..)` statement for a later frame
/// (outside `moving`, the block being moved), else at the end of the function. Every frame line
/// counts, not only those with an `is_excute` block: placing a block after a later frame would
/// make it run at that later frame.
fn frame_insertion_point(text: &str, frame: u32, moving: &Range<usize>) -> Result<usize, String> {
    let mut later: Option<usize> = None;
    for site in scan_named_sites(text, "frame", 0..text.len()) {
        if moving.start <= site.span.start && site.span.start < moving.end {
            continue;
        }
        let Some(line) = line_range_at(text, site.span.start) else {
            continue;
        };
        if !text[line.clone()].trim_start().starts_with("frame(") {
            continue;
        }
        let value = site.arg(text, 1).and_then(|value| value.trim().parse::<f32>().ok());
        let Some(value) = value else {
            return Err(format!(
                "a `frame` statement is not a plain number (`{}`), so where frame {frame} goes is \
                 unclear",
                text[site.span.clone()].trim()
            ));
        };
        if crate::data::script_frame(value) > frame && later.is_none_or(|at| line.start < at) {
            later = Some(line.start);
        }
    }
    if let Some(at) = later {
        return Ok(at);
    }
    let close = text.rfind('}').ok_or("the function has no closing brace")?;
    let line = line_range_at(text, close).ok_or("the function has no closing brace")?;
    if !text[line.start..close].trim().is_empty() {
        return Err("the function's closing brace shares its line with a statement".into());
    }
    Ok(line.start)
}

// ── Kinetic speed write-back ────────────────────────────────────────────────

/// The buildable `SET_SPEED_EX` calls in source order.
pub(crate) fn speed_ex_sites(text: &str) -> Vec<MacroSite> {
    scan_macro_sites(text, 0..text.len())
        .into_iter()
        .filter(|site| site.name == "SET_SPEED_EX" && site.args.len() == 4)
        .collect()
}

/// Rewrite only the x/y values of existing `SET_SPEED_EX` calls.
///
/// The kinetic kind and frame are structural/source-owned values. A named kinetic constant is
/// deliberately not decoded to a guessed integer, and moving a call belongs to the source
/// editor/export path rather than this value-only synchronizer. Exact arity and site checks keep
/// malformed dump artifacts and loop-unrolled events from retargeting a later call.
pub fn rewrite_speed_ex(
    text: &str,
    label: &str,
    pristine: &[crate::data::SetSpeedExEvent],
    edited: &[crate::data::SetSpeedExEvent],
) -> Result<(String, SyncReport)> {
    let shape = (
        "`SET_SPEED_EX`",
        "`SET_SPEED_EX`",
        "retunes velocity values",
        speed_ex_sites(text),
    );
    rewrite_points(
        text,
        label,
        shape,
        pristine,
        edited,
        |site, before, now, report, edits| {
            if before.call.kinetic_kind != now.call.kinetic_kind {
                report.skipped.push(format!(
                "{label}: `SET_SPEED_EX` site {} changed kinetic kind — source syncing preserves that structural token",
                before.site
            ));
                return;
            }
            if site.name != crate::data::SetSpeedExCall::FUNC || site.args.len() != 4 {
                report.skipped.push(format!(
                "{label}: `SET_SPEED_EX` site {} no longer has its verified four-argument shape",
                before.site
            ));
                return;
            }
            // Slot 0 is `agent`; x and y are the two following `ToF32` arguments.
            if let Some(span) = site.args.get(1) {
                if let Some(edit) = to_f32_edit(text, span, now.call.speed_x) {
                    edits.push(edit);
                }
            }
            if let Some(span) = site.args.get(2) {
                if let Some(edit) = to_f32_edit(text, span, now.call.speed_y) {
                    edits.push(edit);
                }
            }
        },
    )
}

/// The buildable direct `SET_SPEED` calls and generated helper calls in source order.
pub(crate) fn speed_sites(text: &str) -> Vec<MacroSite> {
    let mut sites = scan_macro_sites(text, 0..text.len());
    sites.extend(scan_named_sites(text, "visionary_set_speed", 0..text.len()));
    sites.sort_by_key(|site| site.span.start);
    sites
        .into_iter()
        .filter(|site| {
            matches!(site.name.as_str(), "SET_SPEED" | "visionary_set_speed")
                && site.args.len() == 3
        })
        .collect()
}

/// Rewrite only the x/y values of existing `SET_SPEED` calls.
pub fn rewrite_speed(
    text: &str,
    label: &str,
    pristine: &[crate::data::SetSpeedEvent],
    edited: &[crate::data::SetSpeedEvent],
) -> Result<(String, SyncReport)> {
    let shape = (
        "`SET_SPEED`",
        "`SET_SPEED`",
        "retunes velocity values",
        speed_sites(text),
    );
    rewrite_points(
        text,
        label,
        shape,
        pristine,
        edited,
        |site, before, now, report, edits| {
            if !matches!(
                site.name.as_str(),
                crate::data::SetSpeedCall::FUNC | "visionary_set_speed"
            ) || site.args.len() != 3
            {
                report.skipped.push(format!(
                    "{label}: `SET_SPEED` site {} no longer has its verified three-argument shape",
                    before.site
                ));
                return;
            }
            if let Some(span) = site.args.get(1) {
                if let Some(edit) = to_f32_edit(text, span, now.call.speed_x) {
                    edits.push(edit);
                }
            }
            if let Some(span) = site.args.get(2) {
                if let Some(edit) = to_f32_edit(text, span, now.call.speed_y) {
                    edits.push(edit);
                }
            }
        },
    )
}

/// The buildable `ADD_SPEED_NO_LIMIT` calls in source order.
pub(crate) fn add_speed_no_limit_sites(text: &str) -> Vec<MacroSite> {
    scan_macro_sites(text, 0..text.len())
        .into_iter()
        .filter(|site| site.name == "ADD_SPEED_NO_LIMIT" && site.args.len() == 3)
        .collect()
}

/// Rewrite only the x/y values of existing `ADD_SPEED_NO_LIMIT` calls.
pub fn rewrite_add_speed_no_limit(
    text: &str,
    label: &str,
    pristine: &[crate::data::AddSpeedNoLimitEvent],
    edited: &[crate::data::AddSpeedNoLimitEvent],
) -> Result<(String, SyncReport)> {
    let shape = (
        "`ADD_SPEED_NO_LIMIT`",
        "`ADD_SPEED_NO_LIMIT`",
        "retunes velocity values",
        add_speed_no_limit_sites(text),
    );
    rewrite_points(
        text,
        label,
        shape,
        pristine,
        edited,
        |site, before, now, report, edits| {
            if site.name != crate::data::AddSpeedNoLimitCall::FUNC || site.args.len() != 3 {
                report.skipped.push(format!(
                "{label}: `ADD_SPEED_NO_LIMIT` site {} no longer has its verified three-argument shape",
                before.site
            ));
                return;
            }
            // Slot 0 is `agent`; x and y are the following `ToF32` arguments.
            if let Some(span) = site.args.get(1) {
                if let Some(edit) = to_f32_edit(text, span, now.call.speed_x) {
                    edits.push(edit);
                }
            }
            if let Some(span) = site.args.get(2) {
                if let Some(edit) = to_f32_edit(text, span, now.call.speed_y) {
                    edits.push(edit);
                }
            }
        },
    )
}

/// The buildable `CORRECT` calls in source order.
pub(crate) fn correct_sites(text: &str) -> Vec<MacroSite> {
    scan_macro_sites(text, 0..text.len())
        .into_iter()
        .filter(|site| site.name == "CORRECT" && site.args.len() == 2)
        .collect()
}

/// Rewrite the authored correction-kind token of existing `CORRECT` calls.
pub fn rewrite_correct(
    text: &str,
    label: &str,
    pristine: &[crate::data::CorrectEvent],
    edited: &[crate::data::CorrectEvent],
) -> Result<(String, SyncReport)> {
    let shape = (
        "`CORRECT`",
        "`CORRECT`",
        "retunes its authored kind token",
        correct_sites(text),
    );
    rewrite_points(
        text,
        label,
        shape,
        pristine,
        edited,
        |site, before, now, report, edits| {
            if site.name != crate::data::CorrectCall::FUNC || site.args.len() != 2 {
                report.skipped.push(format!(
                    "{label}: `CORRECT` site {} no longer has its verified two-argument shape",
                    before.site
                ));
                return;
            }
            if let Some(span) = site.args.get(1) {
                if let Some(edit) = text_edit(text, span, now.call.kind.trim()) {
                    edits.push(edit);
                }
            }
        },
    )
}

/// The buildable `FT_CATCH_STOP` calls in source order.
pub(crate) fn ft_catch_stop_sites(text: &str) -> Vec<MacroSite> {
    scan_macro_sites(text, 0..text.len())
        .into_iter()
        .filter(|site| site.name == "FT_CATCH_STOP" && site.args.len() == 3)
        .collect()
}

/// Rewrite only the two numeric arguments of existing `FT_CATCH_STOP` calls.
pub fn rewrite_ft_catch_stop(
    text: &str,
    label: &str,
    pristine: &[crate::data::FtCatchStopEvent],
    edited: &[crate::data::FtCatchStopEvent],
) -> Result<(String, SyncReport)> {
    let shape = (
        "`FT_CATCH_STOP`",
        "`FT_CATCH_STOP`",
        "retunes its two numeric arguments",
        ft_catch_stop_sites(text),
    );
    rewrite_points(
        text,
        label,
        shape,
        pristine,
        edited,
        |site, before, now, report, edits| {
            if site.name != crate::data::FtCatchStopCall::FUNC || site.args.len() != 3 {
                report.skipped.push(format!(
                "{label}: `FT_CATCH_STOP` site {} no longer has its verified three-argument shape",
                before.site
            ));
                return;
            }
            if let Some(span) = site.args.get(1) {
                if let Some(edit) = to_f32_edit(text, span, now.call.arg1) {
                    edits.push(edit);
                }
            }
            if let Some(span) = site.args.get(2) {
                if let Some(edit) = to_f32_edit(text, span, now.call.arg2) {
                    edits.push(edit);
                }
            }
        },
    )
}

/// The buildable `FT_START_ADJUST_MOTION_FRAME_arg1` calls in source order.
pub(crate) fn ft_start_adjust_motion_frame_sites(text: &str) -> Vec<MacroSite> {
    scan_macro_sites(text, 0..text.len())
        .into_iter()
        .filter(|site| site.name == "FT_START_ADJUST_MOTION_FRAME_arg1" && site.args.len() == 2)
        .collect()
}

/// Rewrite only the numeric payload of existing motion-frame adjustment calls.
pub fn rewrite_ft_start_adjust_motion_frame(
    text: &str,
    label: &str,
    pristine: &[crate::data::FtStartAdjustMotionFrameEvent],
    edited: &[crate::data::FtStartAdjustMotionFrameEvent],
) -> Result<(String, SyncReport)> {
    let sites = ft_start_adjust_motion_frame_sites(text);
    let mut report = SyncReport::default();
    if pristine.len() != sites.len() || edited.len() != pristine.len() {
        report.skipped.push(format!(
            "{label}: source has {} buildable `FT_START_ADJUST_MOTION_FRAME_arg1` call(s), while the editor has {} pristine and {} edited point(s) — malformed or revised calls are source-only",
            sites.len(),
            pristine.len(),
            edited.len()
        ));
        return Ok((text.to_string(), report));
    }

    let mut edits = Vec::new();
    for (index, (before, now)) in pristine.iter().zip(edited).enumerate() {
        if before == now {
            continue;
        }
        if before.site != index || now.site != index {
            report.skipped.push(format!(
                "{label}: `FT_START_ADJUST_MOTION_FRAME_arg1` site {} does not match the flat source order — source syncing refuses positional guessing",
                before.site
            ));
            continue;
        }
        if before.frame != now.frame {
            report.skipped.push(format!(
                "{label}: `FT_START_ADJUST_MOTION_FRAME_arg1` site {} was retimed from frame {} to {} — source syncing only retunes its numeric value",
                before.site, before.frame, now.frame
            ));
            continue;
        }
        let Some(site) = sites.get(index) else {
            continue;
        };
        if site.name != crate::data::FtStartAdjustMotionFrameCall::FUNC || site.args.len() != 2 {
            report.skipped.push(format!(
                "{label}: `FT_START_ADJUST_MOTION_FRAME_arg1` site {} no longer has its verified two-argument shape",
                before.site
            ));
            continue;
        }
        if let Some(span) = site.args.get(1) {
            if let Some(edit) = to_f32_edit(text, span, now.call.value) {
                edits.push(edit);
            }
        }
    }
    report.changed = edits.len();
    Ok((apply(text, edits), report))
}

/// Sync edited motion-frame adjustment values into the project's `game_` function.
pub fn sync_ft_start_adjust_motion_frame(
    index: &SourceIndex,
    fighter: &str,
    move_name: &str,
    pristine: &[crate::data::FtStartAdjustMotionFrameEvent],
    edited: &[crate::data::FtStartAdjustMotionFrameEvent],
) -> Result<SyncReport> {
    let script_name = crate::acmd::acmd_script_name("game", move_name);
    sync_script(index, fighter, &script_name, |body| {
        rewrite_ft_start_adjust_motion_frame(
            body,
            &format!("{fighter}/{move_name}"),
            pristine,
            edited,
        )
    })
}

/// The verified direct `MotionModule::set_rate` calls in source order.
pub(crate) fn motion_module_set_rate_sites(text: &str) -> Vec<MacroSite> {
    scan_named_sites(
        text,
        crate::data::MotionModuleSetRateCall::FUNC,
        0..text.len(),
    )
    .into_iter()
    .filter(|site| {
        site.args.len() == 2
            && site
                .arg(text, 0)
                .is_some_and(|value| crate::acmd::is_module_accessor(value.trim()))
            && site
                .arg(text, 1)
                .and_then(|value| value.trim().parse::<f32>().ok())
                .is_some_and(|value| value.is_finite() && value >= 0.0)
    })
    .collect()
}

/// Rewrite only the numeric payload of existing direct `MotionModule::set_rate` calls.
pub fn rewrite_motion_module_set_rate(
    text: &str,
    label: &str,
    pristine: &[crate::data::MotionModuleSetRateEvent],
    edited: &[crate::data::MotionModuleSetRateEvent],
) -> Result<(String, SyncReport)> {
    let shape = (
        "`MotionModule::set_rate`",
        "`MotionModule::set_rate`",
        "retunes its numeric rate",
        motion_module_set_rate_sites(text),
    );
    rewrite_points(
        text,
        label,
        shape,
        pristine,
        edited,
        |site, before, now, report, edits| {
            if site.name != crate::data::MotionModuleSetRateCall::FUNC
                || site.args.len() != 2
                || site
                    .arg(text, 0)
                    .is_none_or(|value| !crate::acmd::is_module_accessor(value.trim()))
            {
                report.skipped.push(format!(
                "{label}: `MotionModule::set_rate` site {} no longer has its verified receiver/arity shape",
                before.site
            ));
                return;
            }
            if !(now.call.rate.is_finite() && now.call.rate >= 0.0) {
                report.skipped.push(format!(
                "{label}: `MotionModule::set_rate` site {} has a non-finite or negative rate — source syncing leaves the call intact",
                before.site
            ));
                return;
            }
            if let Some(span) = site.args.get(1) {
                if let Some(edit) = to_f32_edit(text, span, now.call.rate) {
                    edits.push(edit);
                }
            }
        },
    )
}

/// The verified direct `MotionModule::set_helper_calculation` calls in source order.
pub(crate) fn motion_module_set_helper_calculation_sites(text: &str) -> Vec<MacroSite> {
    scan_named_sites(
        text,
        crate::data::MotionModuleSetHelperCalculationCall::FUNC,
        0..text.len(),
    )
    .into_iter()
    .filter(|site| {
        site.args.len() == 2
            && site
                .arg(text, 0)
                .is_some_and(|value| crate::acmd::is_module_accessor(value.trim()))
            && site
                .arg(text, 1)
                .is_some_and(|value| matches!(value.trim(), "false" | "true"))
    })
    .collect()
}

/// Rewrite only the boolean payload of existing direct helper-calculation calls.
pub fn rewrite_motion_module_set_helper_calculation(
    text: &str,
    label: &str,
    pristine: &[crate::data::MotionModuleSetHelperCalculationEvent],
    edited: &[crate::data::MotionModuleSetHelperCalculationEvent],
) -> Result<(String, SyncReport)> {
    let shape = (
        "`MotionModule::set_helper_calculation`",
        "`MotionModule::set_helper_calculation`",
        "retunes its boolean value",
        motion_module_set_helper_calculation_sites(text),
    );
    rewrite_points(
        text,
        label,
        shape,
        pristine,
        edited,
        |site, before, now, report, edits| {
            if site.name != crate::data::MotionModuleSetHelperCalculationCall::FUNC
                || site.args.len() != 2
                || site
                    .arg(text, 0)
                    .is_none_or(|value| !crate::acmd::is_module_accessor(value.trim()))
                || site
                    .arg(text, 1)
                    .is_none_or(|value| !matches!(value.trim(), "false" | "true"))
            {
                report.skipped.push(format!(
                "{label}: `MotionModule::set_helper_calculation` site {} no longer has its verified receiver/arity/boolean shape",
                before.site
            ));
                return;
            }
            if let Some(span) = site.args.get(1) {
                if let Some(edit) = bool_edit(text, span, now.call.enabled) {
                    edits.push(edit);
                }
            }
        },
    )
}

/// Sync edited helper-calculation booleans into the project's `game_` function.
pub fn sync_motion_module_set_helper_calculation(
    index: &SourceIndex,
    fighter: &str,
    move_name: &str,
    pristine: &[crate::data::MotionModuleSetHelperCalculationEvent],
    edited: &[crate::data::MotionModuleSetHelperCalculationEvent],
) -> Result<SyncReport> {
    let script_name = crate::acmd::acmd_script_name("game", move_name);
    sync_script(index, fighter, &script_name, |body| {
        rewrite_motion_module_set_helper_calculation(
            body,
            &format!("{fighter}/{move_name}"),
            pristine,
            edited,
        )
    })
}

fn valid_motion_module_part_kind_token(value: &str) -> bool {
    let value = value.trim();
    value.parse::<i64>().is_ok()
        || value.strip_prefix('*').is_some_and(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
        })
}

/// The verified direct `MotionModule::set_rate_partial` calls in source order.
pub(crate) fn motion_module_set_rate_partial_sites(text: &str) -> Vec<MacroSite> {
    scan_named_sites(
        text,
        crate::data::MotionModuleSetRatePartialCall::FUNC,
        0..text.len(),
    )
    .into_iter()
    .filter(|site| {
        site.args.len() == 3
            && site
                .arg(text, 0)
                .is_some_and(|value| crate::acmd::is_module_accessor(value.trim()))
            && site
                .arg(text, 1)
                .is_some_and(valid_motion_module_part_kind_token)
            && site
                .arg(text, 2)
                .and_then(|value| value.trim().parse::<f32>().ok())
                .is_some_and(|value| value.is_finite() && value >= 0.0)
    })
    .collect()
}

/// Rewrite only the numeric rate of existing direct `MotionModule::set_rate_partial` calls.
pub fn rewrite_motion_module_set_rate_partial(
    text: &str,
    label: &str,
    pristine: &[crate::data::MotionModuleSetRatePartialEvent],
    edited: &[crate::data::MotionModuleSetRatePartialEvent],
) -> Result<(String, SyncReport)> {
    let shape = (
        "`MotionModule::set_rate_partial`",
        "`MotionModule::set_rate_partial`",
        "retunes its numeric rate",
        motion_module_set_rate_partial_sites(text),
    );
    rewrite_points(
        text,
        label,
        shape,
        pristine,
        edited,
        |site, before, now, report, edits| {
            if before.call.part_kind != now.call.part_kind {
                report.skipped.push(format!(
                "{label}: `MotionModule::set_rate_partial` site {} changed part kind — source syncing only retunes its numeric rate",
                before.site
            ));
                return;
            }
            if site.name != crate::data::MotionModuleSetRatePartialCall::FUNC
                || site.args.len() != 3
                || site
                    .arg(text, 0)
                    .is_none_or(|value| !crate::acmd::is_module_accessor(value.trim()))
                || site
                    .arg(text, 1)
                    .is_none_or(|value| value.trim() != before.call.part_kind.trim())
            {
                report.skipped.push(format!(
                "{label}: `MotionModule::set_rate_partial` site {} no longer has its verified receiver/part-kind/arity shape",
                before.site
            ));
                return;
            }
            if !(now.call.rate.is_finite() && now.call.rate >= 0.0) {
                report.skipped.push(format!(
                "{label}: `MotionModule::set_rate_partial` site {} has a non-finite or negative rate — source syncing leaves the call intact",
                before.site
            ));
                return;
            }
            if let Some(span) = site.args.get(2) {
                if let Some(edit) = to_f32_edit(text, span, now.call.rate) {
                    edits.push(edit);
                }
            }
        },
    )
}

/// Sync edited direct `MotionModule::set_rate_partial` values into the project's `game_` function.
pub fn sync_motion_module_set_rate_partial(
    index: &SourceIndex,
    fighter: &str,
    move_name: &str,
    pristine: &[crate::data::MotionModuleSetRatePartialEvent],
    edited: &[crate::data::MotionModuleSetRatePartialEvent],
) -> Result<SyncReport> {
    let script_name = crate::acmd::acmd_script_name("game", move_name);
    sync_script(index, fighter, &script_name, |body| {
        rewrite_motion_module_set_rate_partial(
            body,
            &format!("{fighter}/{move_name}"),
            pristine,
            edited,
        )
    })
}

/// Sync edited direct `MotionModule::set_rate_partial` values into the project's `expression_`
/// function. The rewrite is shared with the `game_` path; only the category anchor differs.
pub fn sync_expression_motion_module_set_rate_partial(
    index: &SourceIndex,
    fighter: &str,
    move_name: &str,
    pristine: &[crate::data::MotionModuleSetRatePartialEvent],
    edited: &[crate::data::MotionModuleSetRatePartialEvent],
) -> Result<SyncReport> {
    let script_name = crate::acmd::acmd_script_name("expression", move_name);
    sync_script(index, fighter, &script_name, |body| {
        rewrite_motion_module_set_rate_partial(
            body,
            &format!("{fighter}/{move_name}"),
            pristine,
            edited,
        )
    })
}

/// The verified direct `MotionModule::set_frame_partial` calls in source order.
///
/// Both authored arities are buildable: the corpus's three-argument form and the native
/// four-argument form. Which one a site uses is part of its shape, so the rewrite below can
/// refuse a change that would have to add or remove an argument.
pub(crate) fn motion_module_set_frame_partial_sites(text: &str) -> Vec<MacroSite> {
    scan_named_sites(
        text,
        crate::data::MotionModuleSetFramePartialCall::FUNC,
        0..text.len(),
    )
    .into_iter()
    .filter(|site| {
        (site.args.len() == 3 || site.args.len() == 4)
            && site
                .arg(text, 0)
                .is_some_and(|value| crate::acmd::is_module_accessor(value.trim()))
            && site
                .arg(text, 1)
                .is_some_and(valid_motion_module_part_kind_token)
            && site
                .arg(text, 2)
                .and_then(|value| value.trim().parse::<f32>().ok())
                .is_some_and(|value| value.is_finite() && value >= 0.0)
            && site
                .arg(text, 3)
                .is_none_or(|value| matches!(value.trim(), "true" | "false"))
    })
    .collect()
}

/// Rewrite only the seek frame and the authored `sync` of existing direct
/// `MotionModule::set_frame_partial` calls.
///
/// Adding or removing `sync` is structural — it changes the call's arity, and an omitted `sync`
/// is not the same source as an authored `true` even though the game runs them identically. Those
/// edits are reported rather than written.
pub fn rewrite_motion_module_set_frame_partial(
    text: &str,
    label: &str,
    pristine: &[crate::data::MotionModuleSetFramePartialEvent],
    edited: &[crate::data::MotionModuleSetFramePartialEvent],
) -> Result<(String, SyncReport)> {
    let shape = (
        "`MotionModule::set_frame_partial`",
        "`MotionModule::set_frame_partial`",
        "retunes its seek frame and sync flag",
        motion_module_set_frame_partial_sites(text),
    );
    rewrite_points(
        text,
        label,
        shape,
        pristine,
        edited,
        |site, before, now, report, edits| {
            if before.call.part_kind != now.call.part_kind {
                report.skipped.push(format!(
                "{label}: `MotionModule::set_frame_partial` site {} changed part kind — source syncing only retunes its seek frame and sync flag",
                before.site
            ));
                return;
            }
            if before.call.sync.is_some() != now.call.sync.is_some() {
                report.skipped.push(format!(
                "{label}: `MotionModule::set_frame_partial` site {} added or removed its `sync` argument — source syncing does not change a call's arity",
                before.site
            ));
                return;
            }
            if site.name != crate::data::MotionModuleSetFramePartialCall::FUNC
                || site.args.len() != usize::from(before.call.sync.is_some()) + 3
                || site
                    .arg(text, 0)
                    .is_none_or(|value| !crate::acmd::is_module_accessor(value.trim()))
                || site
                    .arg(text, 1)
                    .is_none_or(|value| value.trim() != before.call.part_kind.trim())
            {
                report.skipped.push(format!(
                "{label}: `MotionModule::set_frame_partial` site {} no longer has its verified receiver/part-kind/arity shape",
                before.site
            ));
                return;
            }
            if !(now.call.frame.is_finite() && now.call.frame >= 0.0) {
                report.skipped.push(format!(
                "{label}: `MotionModule::set_frame_partial` site {} has a non-finite or negative seek frame — source syncing leaves the call intact",
                before.site
            ));
                return;
            }
            if let Some(span) = site.args.get(2) {
                if let Some(edit) = to_f32_edit(text, span, now.call.frame) {
                    edits.push(edit);
                }
            }
            if let (Some(span), Some(sync)) = (site.args.get(3), now.call.sync) {
                if let Some(edit) = bool_edit(text, span, sync) {
                    edits.push(edit);
                }
            }
        },
    )
}

/// Sync edited direct `MotionModule::set_frame_partial` values into the project's `game_`
/// function.
pub fn sync_motion_module_set_frame_partial(
    index: &SourceIndex,
    fighter: &str,
    move_name: &str,
    pristine: &[crate::data::MotionModuleSetFramePartialEvent],
    edited: &[crate::data::MotionModuleSetFramePartialEvent],
) -> Result<SyncReport> {
    let script_name = crate::acmd::acmd_script_name("game", move_name);
    sync_script(index, fighter, &script_name, |body| {
        rewrite_motion_module_set_frame_partial(
            body,
            &format!("{fighter}/{move_name}"),
            pristine,
            edited,
        )
    })
}

/// Sync edited direct `MotionModule::set_frame_partial` values into the project's `expression_`
/// function. The rewrite is shared with the `game_` path; only the category anchor differs.
pub fn sync_expression_motion_module_set_frame_partial(
    index: &SourceIndex,
    fighter: &str,
    move_name: &str,
    pristine: &[crate::data::MotionModuleSetFramePartialEvent],
    edited: &[crate::data::MotionModuleSetFramePartialEvent],
) -> Result<SyncReport> {
    let script_name = crate::acmd::acmd_script_name("expression", move_name);
    sync_script(index, fighter, &script_name, |body| {
        rewrite_motion_module_set_frame_partial(
            body,
            &format!("{fighter}/{move_name}"),
            pristine,
            edited,
        )
    })
}

// ── Kinetic point write-back ────────────────────────────────────────────────

/// The buildable `CLR_SPEED` calls and generated helper calls in source order.
pub(crate) fn clr_speed_sites(text: &str) -> Vec<MacroSite> {
    let mut sites = scan_macro_sites(text, 0..text.len());
    sites.extend(scan_named_sites(text, "visionary_clr_speed", 0..text.len()));
    sites.sort_by_key(|site| site.span.start);
    sites
        .into_iter()
        .filter(|site| {
            matches!(site.name.as_str(), "CLR_SPEED" | "visionary_clr_speed")
                && site.args.len() == 2
        })
        .collect()
}

/// Rewrite the authored kinetic-ID token of existing `CLR_SPEED` calls.
pub fn rewrite_clr_speed(
    text: &str,
    label: &str,
    pristine: &[crate::data::ClrSpeedEvent],
    edited: &[crate::data::ClrSpeedEvent],
) -> Result<(String, SyncReport)> {
    let shape = (
        "`CLR_SPEED`",
        "`CLR_SPEED`",
        "retunes its authored kinetic token",
        clr_speed_sites(text),
    );
    rewrite_points(
        text,
        label,
        shape,
        pristine,
        edited,
        |site, before, now, report, edits| {
            if !matches!(
                site.name.as_str(),
                crate::data::ClrSpeedCall::FUNC | "visionary_clr_speed"
            ) || site.args.len() != 2
            {
                report.skipped.push(format!(
                    "{label}: `CLR_SPEED` site {} no longer has its verified two-argument shape",
                    before.site
                ));
                return;
            }
            if now.call.kinetic_kind.trim().is_empty() {
                report.skipped.push(format!(
                "{label}: `CLR_SPEED` site {} has an empty kinetic kind — source syncing leaves the authored token intact",
                before.site
            ));
                return;
            }
            if let Some(span) = site.args.get(1) {
                if let Some(edit) = text_edit(text, span, now.call.kinetic_kind.trim()) {
                    edits.push(edit);
                }
            }
        },
    )
}

/// Whether a direct `KineticModule::change_kinetic` receiver is one of the measured source forms.
/// The HDR dump binds `boma` to `agent.boma()` at the top of the function; generated source uses
/// `agent.module_accessor`, so this receiver is normalized only for parsing and source syncing.
fn is_change_kinetic_receiver(value: &str) -> bool {
    crate::acmd::is_module_accessor(value.trim())
}

/// The verified direct `KineticModule::change_kinetic` calls in source order.
pub(crate) fn change_kinetic_sites(text: &str) -> Vec<MacroSite> {
    scan_named_sites(text, crate::data::ChangeKineticCall::FUNC, 0..text.len())
        .into_iter()
        .filter(|site| {
            site.args.len() == 2
                && site.arg(text, 0).is_some_and(is_change_kinetic_receiver)
                && site
                    .arg(text, 1)
                    .is_some_and(|value| !value.trim().is_empty())
        })
        .collect()
}

/// Rewrite only the authored kinetic-type token of an existing direct call.
pub fn rewrite_change_kinetic(
    text: &str,
    label: &str,
    pristine: &[crate::data::ChangeKineticEvent],
    edited: &[crate::data::ChangeKineticEvent],
) -> Result<(String, SyncReport)> {
    let shape = (
        "`KineticModule::change_kinetic`",
        "`KineticModule::change_kinetic`",
        "retunes its authored kinetic token",
        change_kinetic_sites(text),
    );
    rewrite_points(
        text,
        label,
        shape,
        pristine,
        edited,
        |site, before, now, report, edits| {
            if site.name != crate::data::ChangeKineticCall::FUNC || site.args.len() != 2 {
                report.skipped.push(format!(
                "{label}: `KineticModule::change_kinetic` site {} no longer has its verified two-argument shape",
                before.site
            ));
                return;
            }
            if site
                .arg(text, 0)
                .is_none_or(|value| !is_change_kinetic_receiver(value))
            {
                report.skipped.push(format!(
                "{label}: `KineticModule::change_kinetic` site {} no longer targets a measured module receiver",
                before.site
            ));
                return;
            }
            if now.call.kinetic_type.trim().is_empty() {
                report.skipped.push(format!(
                "{label}: `KineticModule::change_kinetic` site {} has an empty kinetic type — source syncing leaves the authored token intact",
                before.site
            ));
                return;
            }
            if let Some(span) = site.args.get(1) {
                if let Some(edit) = text_edit(text, span, now.call.kinetic_type.trim()) {
                    edits.push(edit);
                }
            }
        },
    )
}

/// Whether a direct kinetic-energy receiver is one of the measured source forms.
fn is_kinetic_energy_receiver(value: &str) -> bool {
    crate::acmd::is_module_accessor(value.trim())
}

/// The verified direct kinetic-energy toggle calls in source order.
pub(crate) fn kinetic_energy_sites(text: &str) -> Vec<MacroSite> {
    let mut sites = Vec::new();
    for action in [
        crate::data::KineticEnergyAction::Suspend,
        crate::data::KineticEnergyAction::Resume,
        crate::data::KineticEnergyAction::Enable,
        crate::data::KineticEnergyAction::Unable,
    ] {
        sites.extend(scan_named_sites(text, action.func(), 0..text.len()));
    }
    sites.sort_by_key(|site| site.span.start);
    sites
        .into_iter()
        .filter(|site| {
            site.args.len() == 2
                && site.arg(text, 0).is_some_and(is_kinetic_energy_receiver)
                && site
                    .arg(text, 1)
                    .is_some_and(|value| !value.trim().is_empty())
        })
        .collect()
}

/// Rewrite only the authored energy-ID token of existing direct kinetic-energy calls. The source
/// receiver and operation remain structural; HDR's `boma` spelling is retained.
pub fn rewrite_kinetic_energy(
    text: &str,
    label: &str,
    pristine: &[crate::data::KineticEnergyEvent],
    edited: &[crate::data::KineticEnergyEvent],
) -> Result<(String, SyncReport)> {
    let shape = (
        "direct kinetic-energy",
        "direct kinetic-energy",
        "retunes its authored ID token",
        kinetic_energy_sites(text),
    );
    rewrite_points(
        text,
        label,
        shape,
        pristine,
        edited,
        |site, before, now, report, edits| {
            if before.call.action != now.call.action {
                report.skipped.push(format!(
                "{label}: direct kinetic-energy site {} changed between suspend and resume — source syncing preserves the authored operation",
                before.site
            ));
                return;
            }
            if site.name != before.call.func()
                || site.args.len() != 2
                || site
                    .arg(text, 0)
                    .is_none_or(|value| !is_kinetic_energy_receiver(value))
                || site
                    .arg(text, 1)
                    .is_none_or(|value| value.trim().is_empty())
            {
                report.skipped.push(format!(
                "{label}: direct kinetic-energy site {} no longer has its verified receiver/operation/ID shape",
                before.site
            ));
                return;
            }
            if now.call.kinetic_energy_id.trim().is_empty() {
                report.skipped.push(format!(
                "{label}: direct kinetic-energy site {} has an empty energy ID — source syncing leaves the authored token intact",
                before.site
            ));
                return;
            }
            if let Some(span) = site.args.get(1) {
                if let Some(edit) = text_edit(text, span, now.call.kinetic_energy_id.trim()) {
                    edits.push(edit);
                }
            }
        },
    )
}

fn valid_work_flag_token(value: &str) -> bool {
    let value = value.trim();
    value.parse::<i64>().is_ok()
        || value.strip_prefix('*').is_some_and(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
        })
}

fn valid_work_module_identifier(value: &str) -> bool {
    let mut chars = value.trim().chars();
    chars.next().is_some_and(|character| {
        (character.is_ascii_alphabetic() || character == '_')
            && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
    })
}

fn valid_work_module_slot_token(value: &str) -> bool {
    let value = value.trim();
    valid_work_flag_token(value)
        || value
            .strip_prefix('*')
            .is_some_and(valid_work_module_identifier)
        || valid_work_module_identifier(value)
}

fn valid_work_module_int64_value(value: &str) -> bool {
    let value = value.trim();
    valid_work_module_slot_token(value)
        || value
            .strip_prefix("hash40(\"")
            .and_then(|value| value.strip_suffix("\") as i64"))
            .is_some_and(|value| !value.is_empty() && !value.contains('"'))
}

/// The verified direct `WorkModule::on_flag` / `off_flag` calls in source order.
pub(crate) fn work_flag_sites(text: &str) -> Vec<MacroSite> {
    let mut sites = Vec::new();
    for action in [
        crate::data::WorkFlagAction::On,
        crate::data::WorkFlagAction::Off,
    ] {
        sites.extend(scan_named_sites(text, action.func(), 0..text.len()));
    }
    sites.sort_by_key(|site| site.span.start);
    sites
        .into_iter()
        .filter(|site| {
            site.args.len() == 2
                && site
                    .arg(text, 0)
                    .is_some_and(|value| crate::acmd::is_module_accessor(value.trim()))
                && site.arg(text, 1).is_some_and(valid_work_flag_token)
        })
        .collect()
}

/// Rewrite only the authored flag token of existing direct WorkModule flag calls.
pub fn rewrite_work_flags(
    text: &str,
    label: &str,
    pristine: &[crate::data::WorkFlagEvent],
    edited: &[crate::data::WorkFlagEvent],
) -> Result<(String, SyncReport)> {
    let shape = (
        "direct WorkModule flag",
        "WorkModule flag",
        "retunes its authored flag token",
        work_flag_sites(text),
    );
    rewrite_points(
        text,
        label,
        shape,
        pristine,
        edited,
        |site, before, now, report, edits| {
            if before.call.action != now.call.action {
                report.skipped.push(format!(
                "{label}: WorkModule flag site {} changed between on_flag and off_flag — source syncing preserves the authored operation",
                before.site
            ));
                return;
            }
            if site.name != before.call.func()
                || site.args.len() != 2
                || site
                    .arg(text, 0)
                    .is_none_or(|value| !crate::acmd::is_module_accessor(value.trim()))
                || site
                    .arg(text, 1)
                    .is_none_or(|value| value.trim() != before.call.flag.trim())
            {
                report.skipped.push(format!(
                "{label}: WorkModule flag site {} no longer has its verified receiver/operation/flag shape",
                before.site
            ));
                return;
            }
            if !valid_work_flag_token(&now.call.flag) {
                report.skipped.push(format!(
                "{label}: WorkModule flag site {} has an unsupported or empty flag token — source syncing leaves the authored token intact",
                before.site
            ));
                return;
            }
            if let Some(span) = site.args.get(1) {
                if let Some(edit) = text_edit(text, span, now.call.flag.trim()) {
                    edits.push(edit);
                }
            }
        },
    )
}

/// Return whether a direct transition-term operation uses a measured receiver spelling.
fn valid_work_transition_term_receiver(function: &str, receiver: &str) -> bool {
    match function {
        "WorkModule::enable_transition_term_group"
        | "WorkModule::unable_transition_term_group_ex" => crate::acmd::is_agent_module_accessor(receiver),
        "WorkModule::enable_transition_term" | "WorkModule::unable_transition_term" => {
            crate::acmd::is_module_accessor(receiver)
        }
        _ => false,
    }
}

/// The verified direct WorkModule transition-term and transition-term-group calls in source
/// order.
pub(crate) fn work_transition_term_sites(text: &str) -> Vec<MacroSite> {
    let mut sites = Vec::new();
    for action in [
        crate::data::WorkTransitionTermAction::Enable,
        crate::data::WorkTransitionTermAction::Unable,
        crate::data::WorkTransitionTermAction::EnableGroup,
        crate::data::WorkTransitionTermAction::UnableGroupEx,
    ] {
        sites.extend(scan_named_sites(text, action.func(), 0..text.len()));
    }
    sites.sort_by_key(|site| site.span.start);
    sites
        .into_iter()
        .filter(|site| {
            site.args.len() == 2
                && site.arg(text, 0).is_some_and(|value| {
                    valid_work_transition_term_receiver(&site.name, value.trim())
                })
                && site.arg(text, 1).is_some_and(valid_work_flag_token)
        })
        .collect()
}

/// Rewrite only the authored transition-term or group token of existing direct WorkModule calls.
pub fn rewrite_work_transition_terms(
    text: &str,
    label: &str,
    pristine: &[crate::data::WorkTransitionTermEvent],
    edited: &[crate::data::WorkTransitionTermEvent],
) -> Result<(String, SyncReport)> {
    let shape = (
        "direct WorkModule transition-term",
        "WorkModule transition-term",
        "retunes its authored transition term",
        work_transition_term_sites(text),
    );
    rewrite_points(
        text,
        label,
        shape,
        pristine,
        edited,
        |site, before, now, report, edits| {
            if before.call.action != now.call.action {
                report.skipped.push(format!(
                "{label}: WorkModule transition-term site {} changed between enable and unable — source syncing preserves the authored operation",
                before.site
            ));
                return;
            }
            if before.call.receiver != now.call.receiver {
                report.skipped.push(format!(
                "{label}: WorkModule transition-term site {} changed receiver — source syncing preserves the authored receiver",
                before.site
            ));
                return;
            }
            if site.name != before.call.func()
                || site.args.len() != 2
                || site.arg(text, 0).is_none_or(|value| {
                    value.trim() != before.call.receiver.token()
                        || !valid_work_transition_term_receiver(&site.name, value.trim())
                })
                || site
                    .arg(text, 1)
                    .is_none_or(|value| value.trim() != before.call.transition_term.trim())
            {
                report.skipped.push(format!(
                "{label}: WorkModule transition-term site {} no longer has its verified receiver/operation/term shape",
                before.site
            ));
                return;
            }
            if !valid_work_flag_token(&now.call.transition_term) {
                report.skipped.push(format!(
                "{label}: WorkModule transition-term site {} has an unsupported or empty transition term — source syncing leaves the authored token intact",
                before.site
            ));
                return;
            }
            if let Some(span) = site.args.get(1) {
                if let Some(edit) = text_edit(text, span, now.call.transition_term.trim()) {
                    edits.push(edit);
                }
            }
        },
    )
}

/// The verified direct `WorkModule::inc_int` calls in source order. Standard scripts use
/// `agent.module_accessor`; HDR scripts use `boma`.
pub(crate) fn work_module_inc_int_sites(text: &str) -> Vec<MacroSite> {
    scan_named_sites(text, crate::data::WorkModuleIncIntCall::FUNC, 0..text.len())
        .into_iter()
        .filter(|site| {
            site.args.len() == 2
                && site
                    .arg(text, 0)
                    .is_some_and(|value| crate::acmd::is_module_accessor(value.trim()))
                && site.arg(text, 1).is_some_and(valid_work_flag_token)
        })
        .collect()
}

/// Rewrite only the authored slot token of existing direct `WorkModule::inc_int` calls.
pub fn rewrite_work_module_inc_int(
    text: &str,
    label: &str,
    pristine: &[crate::data::WorkModuleIncIntEvent],
    edited: &[crate::data::WorkModuleIncIntEvent],
) -> Result<(String, SyncReport)> {
    let shape = (
        "direct WorkModule::inc_int",
        "WorkModule::inc_int",
        "retunes its authored slot",
        work_module_inc_int_sites(text),
    );
    rewrite_points(
        text,
        label,
        shape,
        pristine,
        edited,
        |site, before, now, report, edits| {
            if site.name != crate::data::WorkModuleIncIntCall::FUNC
                || site.args.len() != 2
                || site
                    .arg(text, 0)
                    .is_none_or(|value| !crate::acmd::is_module_accessor(value.trim()))
                || site
                    .arg(text, 1)
                    .is_none_or(|value| value.trim() != before.call.slot.trim())
            {
                report.skipped.push(format!(
                "{label}: WorkModule::inc_int site {} no longer has its verified receiver/arity/slot shape",
                before.site
            ));
                return;
            }
            if !valid_work_flag_token(&now.call.slot) {
                report.skipped.push(format!(
                "{label}: WorkModule::inc_int site {} has an unsupported or empty slot token — source syncing leaves the authored call intact",
                before.site
            ));
                return;
            }
            if let Some(span) = site.args.get(1) {
                if let Some(edit) = text_edit(text, span, now.call.slot.trim()) {
                    edits.push(edit);
                }
            }
        },
    )
}

/// The verified direct `WorkModule::set_int` / `set_float` / `set_int64` calls in source order.
/// The set_int64 corpus is measured in both standard `agent.module_accessor` and HDR `boma`
/// receiver forms; preserving the receiver keeps export and source sync lossless.
pub(crate) fn work_module_set_sites(text: &str) -> Vec<MacroSite> {
    let mut sites = Vec::new();
    for kind in [
        crate::data::WorkModuleSetKind::Int,
        crate::data::WorkModuleSetKind::Float,
        crate::data::WorkModuleSetKind::Int64,
    ] {
        sites.extend(scan_named_sites(text, kind.func(), 0..text.len()));
    }
    sites.sort_by_key(|site| site.span.start);
    sites
        .into_iter()
        .filter(|site| {
            let kind = match site.name.as_str() {
                "WorkModule::set_int" => crate::data::WorkModuleSetKind::Int,
                "WorkModule::set_float" => crate::data::WorkModuleSetKind::Float,
                "WorkModule::set_int64" => crate::data::WorkModuleSetKind::Int64,
                _ => return false,
            };
            site.args.len() == 3
                && site
                    .arg(text, 0)
                    .is_some_and(|value| crate::acmd::is_module_accessor(value.trim()))
                && site
                    .arg(text, 1)
                    .is_some_and(|value| valid_work_module_set_value(value, kind))
                && site.arg(text, 2).is_some_and(|value| {
                    if kind.is_int64() {
                        valid_work_module_slot_token(value)
                    } else {
                        valid_work_flag_token(value)
                    }
                })
        })
        .collect()
}

fn valid_work_module_set_value(value: &str, kind: crate::data::WorkModuleSetKind) -> bool {
    let value = value.trim();
    if kind.is_float() {
        value.parse::<f32>().is_ok_and(f32::is_finite)
    } else if kind.is_int64() {
        valid_work_module_int64_value(value)
    } else {
        valid_work_flag_token(value)
    }
}

/// Rewrite only authored value and slot tokens of existing direct WorkModule setters.
pub fn rewrite_work_module_sets(
    text: &str,
    label: &str,
    pristine: &[crate::data::WorkModuleSetEvent],
    edited: &[crate::data::WorkModuleSetEvent],
) -> Result<(String, SyncReport)> {
    let sites = work_module_set_sites(text);
    let mut report = SyncReport::default();
    if pristine.len() != sites.len() || edited.len() != pristine.len() {
        report.skipped.push(format!(
            "{label}: source has {} buildable direct WorkModule set call(s), while the editor has {} pristine and {} edited point(s) — malformed or unmeasured calls are source-only",
            sites.len(),
            pristine.len(),
            edited.len()
        ));
        return Ok((text.to_string(), report));
    }

    let mut edits = Vec::new();
    for (index, (before, now)) in pristine.iter().zip(edited).enumerate() {
        if before == now {
            continue;
        }
        if before.site != index || now.site != index {
            report.skipped.push(format!(
                "{label}: WorkModule set site {} does not match the flat source order — source syncing refuses positional guessing",
                before.site
            ));
            continue;
        }
        if before.frame != now.frame {
            report.skipped.push(format!(
                "{label}: WorkModule set site {} was retimed from frame {} to {} — source syncing only retunes authored value/slot tokens",
                before.site, before.frame, now.frame
            ));
            continue;
        }
        if before.call.kind != now.call.kind {
            report.skipped.push(format!(
                "{label}: WorkModule set site {} changed setter operation — source syncing preserves the authored operation",
                before.site
            ));
            continue;
        }
        if before.call.receiver.trim() != now.call.receiver.trim() {
            report.skipped.push(format!(
                "{label}: WorkModule set site {} changed receiver from {:?} to {:?} — source syncing preserves the authored receiver",
                before.site,
                before.call.receiver.trim(),
                now.call.receiver.trim()
            ));
            continue;
        }
        let Some(site) = sites.get(index) else {
            continue;
        };
        if site.name != before.call.func()
            || site.args.len() != 3
            || site
                .arg(text, 0)
                .is_none_or(|value| value.trim() != before.call.receiver.trim())
            || site
                .arg(text, 1)
                .is_none_or(|value| value.trim() != before.call.value.trim())
            || site
                .arg(text, 2)
                .is_none_or(|value| value.trim() != before.call.slot.trim())
        {
            report.skipped.push(format!(
                "{label}: WorkModule set site {} no longer has its verified receiver/operation/value/slot shape",
                before.site
            ));
            continue;
        }
        if !crate::acmd::is_module_accessor(now.call.receiver.trim())
            || !valid_work_module_set_value(&now.call.value, now.call.kind)
            || if now.call.kind.is_int64() {
                !valid_work_module_slot_token(&now.call.slot)
            } else {
                !valid_work_flag_token(&now.call.slot)
            }
        {
            report.skipped.push(format!(
                "{label}: WorkModule set site {} has an unsupported or empty value/slot token — source syncing leaves the authored call intact",
                before.site
            ));
            continue;
        }
        if let Some(span) = site.args.get(1) {
            if let Some(edit) = text_edit(text, span, now.call.value.trim()) {
                edits.push(edit);
            }
        }
        if let Some(span) = site.args.get(2) {
            if let Some(edit) = text_edit(text, span, now.call.slot.trim()) {
                edits.push(edit);
            }
        }
    }
    report.changed = edits.len();
    Ok((apply(text, edits), report))
}

/// Whether a direct `KineticModule::add_speed` receiver is one of the measured source forms.
fn is_kinetic_add_speed_receiver(value: &str) -> bool {
    crate::acmd::is_module_accessor(value.trim())
}

fn parse_source_zero_z_vector(value: &str) -> Option<(f32, f32)> {
    let value = value.trim();
    let value = value.strip_prefix('&')?.trim_start();
    let value = value.strip_prefix("Vector3f")?.trim_start();
    let value = value.strip_prefix('{')?.strip_suffix('}')?;
    let fields = split_source_fields(value);
    let [x, y, z] = fields.as_slice() else {
        return None;
    };
    let (x_name, x_value) = x.split_once(':')?;
    let (y_name, y_value) = y.split_once(':')?;
    let (z_name, z_value) = z.split_once(':')?;
    if x_name.trim() != "x" || y_name.trim() != "y" || z_name.trim() != "z" {
        return None;
    }
    let speed_x = x_value.trim().parse::<f32>().ok()?;
    let speed_y = y_value.trim().parse::<f32>().ok()?;
    let speed_z = z_value.trim().parse::<f32>().ok()?;
    (speed_z.is_finite() && speed_z == 0.0).then_some((speed_x, speed_y))
}

fn split_source_fields(text: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                fields.push(text[start..index].trim());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    if !text[start..].trim().is_empty() {
        fields.push(text[start..].trim());
    }
    fields
}

/// The verified direct `KineticModule::add_speed` calls in source order.
pub(crate) fn kinetic_add_speed_sites(text: &str) -> Vec<MacroSite> {
    scan_named_sites(text, crate::data::KineticAddSpeedCall::FUNC, 0..text.len())
        .into_iter()
        .filter(|site| {
            site.args.len() == 2
                && site.arg(text, 0).is_some_and(is_kinetic_add_speed_receiver)
                && site
                    .arg(text, 1)
                    .and_then(parse_source_zero_z_vector)
                    .is_some()
        })
        .collect()
}

/// Rewrite the measured x/y components of existing direct kinetic-vector additions. The source
/// receiver is retained (`agent.module_accessor` or HDR's `boma`); only the verified vector
/// argument is replaced, with the measured zero z component made explicit.
pub fn rewrite_kinetic_add_speed(
    text: &str,
    label: &str,
    pristine: &[crate::data::KineticAddSpeedEvent],
    edited: &[crate::data::KineticAddSpeedEvent],
) -> Result<(String, SyncReport)> {
    let shape = (
        "`KineticModule::add_speed`",
        "`KineticModule::add_speed`",
        "retunes its authored vector",
        kinetic_add_speed_sites(text),
    );
    rewrite_points(
        text,
        label,
        shape,
        pristine,
        edited,
        |site, before, now, report, edits| {
            if site.name != crate::data::KineticAddSpeedCall::FUNC
                || site.args.len() != 2
                || site
                    .arg(text, 0)
                    .is_none_or(|value| !is_kinetic_add_speed_receiver(value))
                || site
                    .arg(text, 1)
                    .and_then(parse_source_zero_z_vector)
                    .is_none()
            {
                report.skipped.push(format!(
                "{label}: `KineticModule::add_speed` site {} no longer has its verified receiver/vector shape",
                before.site
            ));
                return;
            }
            if let Some(span) = site.args.get(1) {
                let value = format!(
                    "&Vector3f{{x: {}, y: {}, z: 0.0}}",
                    crate::acmd::num(now.call.speed_x),
                    crate::acmd::num(now.call.speed_y)
                );
                if let Some(edit) = text_edit(text, span, &value) {
                    edits.push(edit);
                }
            }
        },
    )
}

/// The measured direct `KineticModule::set_consider_ground_friction` calls in source order.
pub(crate) fn kinetic_set_consider_ground_friction_sites(text: &str) -> Vec<MacroSite> {
    scan_named_sites(
        text,
        crate::data::KineticSetConsiderGroundFrictionCall::FUNC,
        0..text.len(),
    )
    .into_iter()
    .filter(|site| {
        site.args.len() == 3
            && site
                .arg(text, 0)
                .is_some_and(|value| crate::acmd::is_module_accessor(value.trim()))
            && site
                .arg(text, 1)
                .is_some_and(|value| matches!(value.trim(), "true" | "false"))
            && site
                .arg(text, 2)
                .is_some_and(|value| !value.trim().is_empty())
    })
    .collect()
}

fn remove_kinetic_set_consider_ground_friction_line(text: &str, site: &MacroSite) -> Replacement {
    let ranges = source_line_ranges(text);
    let Some(line) = ranges
        .iter()
        .find(|range| range.start <= site.span.start && site.span.start < range.end)
    else {
        return Replacement {
            span: site.span.clone(),
            value: String::new(),
        };
    };
    let trimmed = text[line.clone()].trim();
    if trimmed == "KineticModule::set_consider_ground_friction(agent.module_accessor, true, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);"
        || trimmed == "KineticModule::set_consider_ground_friction(agent.module_accessor, false, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);"
        || trimmed == "KineticModule::set_consider_ground_friction(boma, true, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);"
        || trimmed == "KineticModule::set_consider_ground_friction(boma, false, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);"
    {
        Replacement {
            span: line.clone(),
            value: String::new(),
        }
    } else {
        let mut end = site.span.end;
        if text[end..].starts_with(';') {
            end += 1;
        }
        Replacement {
            span: site.span.start..end,
            value: String::new(),
        }
    }
}

fn insert_kinetic_set_consider_ground_friction_line(
    text: &mut String,
    frame: u32,
    call: &crate::data::KineticSetConsiderGroundFrictionCall,
) -> bool {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let receiver = if text.contains("let boma = agent.boma()") {
        "boma"
    } else {
        "agent.module_accessor"
    };
    let call = format!(
        "KineticModule::set_consider_ground_friction({receiver}, {}, {});",
        call.consider_ground_friction, call.kinetic_energy_attribute
    );
    let ranges = source_line_ranges(text);
    let frame_index = ranges.iter().position(|range| {
        frame_literal(&text[range.clone()])
            .is_some_and(|value| value.round().max(1.0) as u32 == frame)
    });

    if let Some(frame_index) = frame_index {
        let frame_range = ranges[frame_index].clone();
        for range in ranges.iter().skip(frame_index + 1) {
            if frame_literal(&text[range.clone()]).is_some() {
                break;
            }
            let line = &text[range.clone()];
            if opens_is_excute(line) {
                let open = range.start + line.find('{').unwrap();
                if let Some(close) = matching_brace(text, open) {
                    let body_indent = ranges
                        .iter()
                        .skip(frame_index + 1)
                        .find(|body| {
                            body.start > open && body.start < close && {
                                let body_text = &text[body.start..body.end];
                                !body_text.trim().is_empty() && !body_text.trim().starts_with('}')
                            }
                        })
                        .map(|body| line_indent(text, body).to_string())
                        .unwrap_or_else(|| format!("{}    ", line_indent(text, range)));
                    insert_styled(text, close, &format!("{body_indent}{call}{newline}"));
                    return true;
                }
            }
        }

        let indent = line_indent(text, &frame_range);
        let block = format!(
            "{indent}if macros::is_excute(agent) {{{newline}{indent}    {call}{newline}{indent}}}{newline}"
        );
        insert_styled(text, frame_range.end, &block);
        return true;
    }

    let insert_at = ranges
        .iter()
        .find(|range| {
            frame_literal(&text[range.start..range.end]).is_some_and(|value| value > frame as f32)
        })
        .map(|range| range.start)
        .or_else(|| text.rfind('}'))
        .unwrap_or(text.len());
    let indent = ranges
        .iter()
        .find(|range| frame_literal(&text[range.start..range.end]).is_some())
        .map(|range| line_indent(text, range).to_string())
        .unwrap_or_else(|| "    ".into());
    let block = format!(
        "{indent}frame(agent.lua_state_agent, {frame}.0);{newline}{indent}if macros::is_excute(agent) {{{newline}{indent}    {call}{newline}{indent}}}{newline}"
    );
    insert_styled(text, insert_at, &block);
    true
}

/// Rewrite direct ground-friction points in a flat `game_` function. Existing values are edited
/// in place; flat additions/removals/retimes use the same frame-ordered structural policy as
/// the argument-less direct kinetic point. Branches, loops, and source-site mismatches are
/// reported instead of guessed into an execution context.
pub fn rewrite_kinetic_set_consider_ground_friction(
    text: &str,
    label: &str,
    pristine: &[crate::data::KineticSetConsiderGroundFrictionEvent],
    edited: &[crate::data::KineticSetConsiderGroundFrictionEvent],
) -> Result<(String, SyncReport)> {
    let sites = kinetic_set_consider_ground_friction_sites(text);
    let parsed = crate::acmd::parse_acmd_script(text);
    fn contains_loop(stmts: &[crate::data::AcmdStmt]) -> bool {
        stmts.iter().any(|stmt| match stmt {
            crate::data::AcmdStmt::Loop { .. } => true,
            crate::data::AcmdStmt::RawBlock { body, .. } => contains_loop(body),
            _ => false,
        })
    }
    let mut report = SyncReport::default();
    let flat_sites = pristine.len() == sites.len()
        && pristine
            .iter()
            .enumerate()
            .all(|(index, event)| event.site == index);
    let flat_edited = edited
        .iter()
        .enumerate()
        .all(|(index, event)| event.site == index);
    if parsed.branch_count() > 0 || contains_loop(&parsed.stmts) || !flat_sites || !flat_edited {
        if pristine != edited {
            report.skipped.push(format!(
                "{label}: set_consider_ground_friction placement changed inside a loop/branch or after a source-site mismatch — source syncing only edits flat point calls"
            ));
        }
        return Ok((text.to_string(), report));
    }

    let old: Vec<u32> = pristine.iter().map(|event| event.frame).collect();
    let new: Vec<u32> = edited.iter().map(|event| event.frame).collect();
    let n = old.len();
    let m = new.len();
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if old[i] == new[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut matched_old = vec![false; n];
    let mut matched_new = vec![false; m];
    let mut matched_pairs = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if old[i] == new[j] {
            matched_old[i] = true;
            matched_new[j] = true;
            matched_pairs.push((i, j));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let mut edits = Vec::new();
    for (old_index, new_index) in &matched_pairs {
        let before = &pristine[*old_index];
        let now = &edited[*new_index];
        if before == now {
            continue;
        }
        let Some(site) = sites.get(*old_index) else {
            continue;
        };
        if site.name != crate::data::KineticSetConsiderGroundFrictionCall::FUNC
            || site.args.len() != 3
            || site
                .arg(text, 0)
                .is_none_or(|value| !crate::acmd::is_module_accessor(value.trim()))
        {
            report.skipped.push(format!(
                "{label}: set_consider_ground_friction site {} no longer has its verified receiver/arity shape",
                before.site
            ));
            continue;
        }
        if before.call.consider_ground_friction != now.call.consider_ground_friction {
            if let Some(span) = site.args.get(1) {
                if let Some(edit) = text_edit(
                    text,
                    span,
                    if now.call.consider_ground_friction {
                        "true"
                    } else {
                        "false"
                    },
                ) {
                    edits.push(edit);
                }
            }
        }
        if before.call.kinetic_energy_attribute != now.call.kinetic_energy_attribute {
            if now.call.kinetic_energy_attribute.trim().is_empty() {
                report.skipped.push(format!(
                    "{label}: set_consider_ground_friction site {} has an empty reserve attribute — source syncing leaves the authored token intact",
                    before.site
                ));
            } else if let Some(span) = site.args.get(2) {
                if let Some(edit) = text_edit(text, span, now.call.kinetic_energy_attribute.trim())
                {
                    edits.push(edit);
                }
            }
        }
    }
    if matched_old.iter().all(|matched| *matched) && matched_new.iter().all(|matched| *matched) {
        report.changed = edits.len();
        return Ok((apply(text, edits), report));
    }

    let removals: Vec<Replacement> = sites
        .iter()
        .enumerate()
        .filter(|(index, _)| !matched_old[*index])
        .map(|(_, site)| remove_kinetic_set_consider_ground_friction_line(text, site))
        .collect();
    edits.extend(removals);
    let mut updated = apply(text, edits);
    report.changed += matched_old.iter().filter(|matched| !**matched).count();
    report.changed += matched_pairs
        .iter()
        .filter(|(old_index, new_index)| pristine[*old_index] != edited[*new_index])
        .count();
    for (index, event) in edited
        .iter()
        .enumerate()
        .filter(|(index, _)| !matched_new[*index])
    {
        if insert_kinetic_set_consider_ground_friction_line(&mut updated, event.frame, &event.call)
        {
            report.changed += 1;
        } else {
            report.skipped.push(format!(
                "{label}: could not insert set_consider_ground_friction point {} at frame {}",
                index, event.frame
            ));
        }
    }
    Ok((updated, report))
}

/// Sync edited direct ground-friction points into the project's `game_` function.
pub fn sync_kinetic_set_consider_ground_friction(
    index: &SourceIndex,
    fighter: &str,
    move_name: &str,
    pristine: &[crate::data::KineticSetConsiderGroundFrictionEvent],
    edited: &[crate::data::KineticSetConsiderGroundFrictionEvent],
) -> Result<SyncReport> {
    let script_name = crate::acmd::acmd_script_name("game", move_name);
    sync_script(index, fighter, &script_name, |body| {
        rewrite_kinetic_set_consider_ground_friction(
            body,
            &format!("{fighter}/{move_name}"),
            pristine,
            edited,
        )
    })
}

/// The buildable argument-less `SET_AIR` calls and generated helper calls in source order.
pub(crate) fn set_air_sites(text: &str) -> Vec<MacroSite> {
    let mut sites = scan_macro_sites(text, 0..text.len());
    sites.extend(scan_named_sites(text, "visionary_set_air", 0..text.len()));
    sites.sort_by_key(|site| site.span.start);
    sites
        .into_iter()
        .filter(|site| {
            matches!(site.name.as_str(), "SET_AIR" | "visionary_set_air") && site.args.len() == 1
        })
        .collect()
}

/// The buildable direct `KineticModule::clear_speed_all` calls in source order.
pub(crate) fn kinetic_clear_speed_all_sites(text: &str) -> Vec<MacroSite> {
    scan_named_sites(
        text,
        crate::data::KineticClearSpeedAllCall::FUNC,
        0..text.len(),
    )
    .into_iter()
    .filter(|site| {
        site.args.len() == 1
            && site
                .arg(text, 0)
                .is_some_and(|value| crate::acmd::is_module_accessor(value.trim()))
    })
    .collect()
}

fn remove_set_air_line(text: &str, site: &MacroSite) -> Replacement {
    let ranges = source_line_ranges(text);
    let Some(line) = ranges
        .iter()
        .find(|range| range.start <= site.span.start && site.span.start < range.end)
    else {
        return Replacement {
            span: site.span.clone(),
            value: String::new(),
        };
    };
    let trimmed = text[line.clone()].trim();
    if is_lone_agent_call(trimmed, "SET_AIR") || is_lone_agent_call(trimmed, "visionary_set_air") {
        Replacement {
            span: line.clone(),
            value: String::new(),
        }
    } else {
        let mut end = site.span.end;
        if text[end..].starts_with(';') {
            end += 1;
        }
        Replacement {
            span: site.span.start..end,
            value: String::new(),
        }
    }
}

fn insert_set_air_line(text: &mut String, frame: u32) -> bool {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let ranges = source_line_ranges(text);
    let frame_index = ranges.iter().position(|range| {
        frame_literal(&text[range.clone()])
            .is_some_and(|value| value.round().max(1.0) as u32 == frame)
    });

    if let Some(frame_index) = frame_index {
        let frame_range = ranges[frame_index].clone();
        for range in ranges.iter().skip(frame_index + 1) {
            if frame_literal(&text[range.clone()]).is_some() {
                break;
            }
            let line = &text[range.clone()];
            if opens_is_excute(line) {
                let open = range.start + line.find('{').unwrap();
                if let Some(close) = matching_brace(text, open) {
                    let body_indent = ranges
                        .iter()
                        .skip(frame_index + 1)
                        .find(|body| {
                            body.start > open && body.start < close && {
                                let body_text = &text[body.start..body.end];
                                !body_text.trim().is_empty() && !body_text.trim().starts_with('}')
                            }
                        })
                        .map(|body| line_indent(text, body).to_string())
                        .unwrap_or_else(|| format!("{}    ", line_indent(text, range)));
                    insert_styled(text,
                        close,
                        &format!("{body_indent}macros::SET_AIR(agent);{newline}"),
                    );
                    return true;
                }
            }
        }

        let indent = line_indent(text, &frame_range);
        let block = format!(
            "{indent}if macros::is_excute(agent) {{{newline}{indent}    macros::SET_AIR(agent);{newline}{indent}}}{newline}"
        );
        insert_styled(text, frame_range.end, &block);
        return true;
    }

    let insert_at = ranges
        .iter()
        .find(|range| {
            frame_literal(&text[range.start..range.end]).is_some_and(|value| value > frame as f32)
        })
        .map(|range| range.start)
        .or_else(|| text.rfind('}'))
        .unwrap_or(text.len());
    let indent = ranges
        .iter()
        .find(|range| frame_literal(&text[range.start..range.end]).is_some())
        .map(|range| line_indent(text, range).to_string())
        .unwrap_or_else(|| "    ".into());
    let block = format!(
        "{indent}frame(agent.lua_state_agent, {frame}.0);{newline}{indent}if macros::is_excute(agent) {{{newline}{indent}    macros::SET_AIR(agent);{newline}{indent}}}{newline}"
    );
    insert_styled(text, insert_at, &block);
    true
}

fn remove_kinetic_clear_speed_all_line(text: &str, site: &MacroSite) -> Replacement {
    let ranges = source_line_ranges(text);
    let Some(line) = ranges
        .iter()
        .find(|range| range.start <= site.span.start && site.span.start < range.end)
    else {
        return Replacement {
            span: site.span.clone(),
            value: String::new(),
        };
    };
    let trimmed = text[line.clone()].trim();
    if matches!(
        trimmed,
        "KineticModule::clear_speed_all(agent.module_accessor);"
            | "KineticModule::clear_speed_all(boma);"
    ) {
        Replacement {
            span: line.clone(),
            value: String::new(),
        }
    } else {
        let mut end = site.span.end;
        if text[end..].starts_with(';') {
            end += 1;
        }
        Replacement {
            span: site.span.start..end,
            value: String::new(),
        }
    }
}

fn insert_kinetic_clear_speed_all_line(text: &mut String, frame: u32) -> bool {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let call = if text.contains("let boma = agent.boma()") {
        "KineticModule::clear_speed_all(boma);"
    } else {
        "KineticModule::clear_speed_all(agent.module_accessor);"
    };
    let ranges = source_line_ranges(text);
    let frame_index = ranges.iter().position(|range| {
        frame_literal(&text[range.clone()])
            .is_some_and(|value| value.round().max(1.0) as u32 == frame)
    });

    if let Some(frame_index) = frame_index {
        let frame_range = ranges[frame_index].clone();
        for range in ranges.iter().skip(frame_index + 1) {
            if frame_literal(&text[range.clone()]).is_some() {
                break;
            }
            let line = &text[range.clone()];
            if opens_is_excute(line) {
                let open = range.start + line.find('{').unwrap();
                if let Some(close) = matching_brace(text, open) {
                    let body_indent = ranges
                        .iter()
                        .skip(frame_index + 1)
                        .find(|body| {
                            body.start > open && body.start < close && {
                                let body_text = &text[body.start..body.end];
                                !body_text.trim().is_empty() && !body_text.trim().starts_with('}')
                            }
                        })
                        .map(|body| line_indent(text, body).to_string())
                        .unwrap_or_else(|| format!("{}    ", line_indent(text, range)));
                    insert_styled(text, close, &format!("{body_indent}{call}{newline}"));
                    return true;
                }
            }
        }

        let indent = line_indent(text, &frame_range);
        let block = format!(
            "{indent}if macros::is_excute(agent) {{{newline}{indent}    {call}{newline}{indent}}}{newline}"
        );
        insert_styled(text, frame_range.end, &block);
        return true;
    }

    let insert_at = ranges
        .iter()
        .find(|range| {
            frame_literal(&text[range.start..range.end]).is_some_and(|value| value > frame as f32)
        })
        .map(|range| range.start)
        .or_else(|| text.rfind('}'))
        .unwrap_or(text.len());
    let indent = ranges
        .iter()
        .find(|range| frame_literal(&text[range.start..range.end]).is_some())
        .map(|range| line_indent(text, range).to_string())
        .unwrap_or_else(|| "    ".into());
    let block = format!(
        "{indent}frame(agent.lua_state_agent, {frame}.0);{newline}{indent}if macros::is_excute(agent) {{{newline}{indent}    {call}{newline}{indent}}}{newline}"
    );
    insert_styled(text, insert_at, &block);
    true
}

/// Rewrite direct `KineticModule::clear_speed_all` presence and frame placement in a flat
/// `game_` function. Structural changes in branches, loops, or mismatched source ordinals are
/// reported rather than placed into an execution context the editor cannot prove.
pub fn rewrite_kinetic_clear_speed_all(
    text: &str,
    label: &str,
    pristine: &[crate::data::KineticClearSpeedAllEvent],
    edited: &[crate::data::KineticClearSpeedAllEvent],
) -> Result<(String, SyncReport)> {
    let sites = kinetic_clear_speed_all_sites(text);
    let parsed = crate::acmd::parse_acmd_script(text);
    fn contains_loop(stmts: &[crate::data::AcmdStmt]) -> bool {
        stmts.iter().any(|stmt| match stmt {
            crate::data::AcmdStmt::Loop { .. } => true,
            crate::data::AcmdStmt::RawBlock { body, .. } => contains_loop(body),
            _ => false,
        })
    }
    let mut report = SyncReport::default();
    let flat_sites = pristine.len() == sites.len()
        && pristine
            .iter()
            .enumerate()
            .all(|(index, event)| event.site == index);
    let flat_edited = edited
        .iter()
        .enumerate()
        .all(|(index, event)| event.site == index);
    let old: Vec<u32> = pristine.iter().map(|event| event.frame).collect();
    let new: Vec<u32> = edited.iter().map(|event| event.frame).collect();
    let has_unsupported_structure =
        parsed.branch_count() > 0 || contains_loop(&parsed.stmts) || !flat_sites || !flat_edited;
    if has_unsupported_structure {
        if pristine != edited {
            report.skipped.push(format!(
                "{label}: KineticModule::clear_speed_all placement changed inside a loop/branch or after a source-site mismatch — source syncing only edits flat point calls"
            ));
        }
        return Ok((text.to_string(), report));
    }

    let n = old.len();
    let m = new.len();
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if old[i] == new[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut matched_old = vec![false; n];
    let mut matched_new = vec![false; m];
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if old[i] == new[j] {
            matched_old[i] = true;
            matched_new[j] = true;
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    if matched_old.iter().all(|matched| *matched) && matched_new.iter().all(|matched| *matched) {
        return Ok((text.to_string(), report));
    }

    let removals: Vec<Replacement> = sites
        .iter()
        .enumerate()
        .filter(|(index, _)| !matched_old[*index])
        .map(|(_, site)| remove_kinetic_clear_speed_all_line(text, site))
        .collect();
    let mut updated = apply(text, removals);
    for (_, event) in edited
        .iter()
        .enumerate()
        .filter(|(index, _)| !matched_new[*index])
    {
        if insert_kinetic_clear_speed_all_line(&mut updated, event.frame) {
            report.changed += 1;
        }
    }
    report.changed += matched_old.iter().filter(|matched| !**matched).count();
    Ok((updated, report))
}

/// Rewrite `SET_AIR` presence and frame placement in a flat `game_` function.
pub fn rewrite_set_air(
    text: &str,
    label: &str,
    pristine: &[crate::data::SetAirEvent],
    edited: &[crate::data::SetAirEvent],
) -> Result<(String, SyncReport)> {
    let sites = set_air_sites(text);
    let parsed = crate::acmd::parse_acmd_script(text);
    fn contains_loop(stmts: &[crate::data::AcmdStmt]) -> bool {
        stmts.iter().any(|stmt| match stmt {
            crate::data::AcmdStmt::Loop { .. } => true,
            crate::data::AcmdStmt::RawBlock { body, .. } => contains_loop(body),
            _ => false,
        })
    }
    let mut report = SyncReport::default();
    let flat_sites = pristine.len() == sites.len()
        && pristine
            .iter()
            .enumerate()
            .all(|(index, event)| event.site == index);
    let flat_edited = edited
        .iter()
        .enumerate()
        .all(|(index, event)| event.site == index);
    let old: Vec<u32> = pristine.iter().map(|event| event.frame).collect();
    let new: Vec<u32> = edited.iter().map(|event| event.frame).collect();
    let has_unsupported_structure =
        parsed.branch_count() > 0 || contains_loop(&parsed.stmts) || !flat_sites || !flat_edited;
    if has_unsupported_structure {
        if pristine != edited {
            report.skipped.push(format!(
                "{label}: SET_AIR placement changed inside a loop/branch or after a source-site mismatch — source syncing only edits flat point calls"
            ));
        }
        return Ok((text.to_string(), report));
    }

    let n = old.len();
    let m = new.len();
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if old[i] == new[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut matched_old = vec![false; n];
    let mut matched_new = vec![false; m];
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if old[i] == new[j] {
            matched_old[i] = true;
            matched_new[j] = true;
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    if matched_old.iter().all(|matched| *matched) && matched_new.iter().all(|matched| *matched) {
        return Ok((text.to_string(), report));
    }

    let removals: Vec<Replacement> = sites
        .iter()
        .enumerate()
        .filter(|(index, _)| !matched_old[*index])
        .map(|(_, site)| remove_set_air_line(text, site))
        .collect();
    let mut updated = apply(text, removals);
    for (_, event) in edited
        .iter()
        .enumerate()
        .filter(|(index, _)| !matched_new[*index])
    {
        if insert_set_air_line(&mut updated, event.frame) {
            report.changed += 1;
        }
    }
    report.changed += matched_old.iter().filter(|matched| !**matched).count();
    Ok((updated, report))
}

// ── Facing-direction point write-back ───────────────────────────────────────

/// The exact argument-less `REVERSE_LR` calls in a source function.
pub(crate) fn reverse_lr_sites(text: &str) -> Vec<MacroSite> {
    scan_macro_sites(text, 0..text.len())
        .into_iter()
        .filter(|site| site.name == "REVERSE_LR" && site.args.len() == 1)
        .collect()
}

/// Insert canonical generated `block` at `at`, re-spelled in the style of `text` (the function
/// being edited): its agent name, macro path, `frame` spelling, indentation and line endings.
/// See [`crate::source_style`].
fn insert_styled(text: &mut String, at: usize, block: &str) {
    let style = crate::source_style::SourceStyle::detect(text);
    let first = block.lines().next().unwrap_or("");
    let base = &first[..first.len() - first.trim_start().len()];
    let styled = style.apply(block, base);
    // Callers aim at a block's closing `}`. Inserting there put the new line after the brace's
    // own indentation — doubling it — and left the brace at column 0. A whole-line block goes
    // in at the start of the brace's line instead.
    let line_start = text[..at].rfind('\n').map_or(0, |n| n + 1);
    let at = if styled.ends_with('\n') && text[line_start..at].trim().is_empty() {
        line_start
    } else {
        at
    };
    text.insert_str(at, &styled);
}

/// Whether `trimmed` is exactly one call of the macro `name` on the agent alone —
/// `macros::SET_AIR(agent);`, `SET_AIR(fighter);` — however the file spells the path and agent.
fn is_lone_agent_call(trimmed: &str, name: &str) -> bool {
    let Some(at) = trimmed.find(&format!("{name}(")) else {
        return false;
    };
    let path = &trimmed[..at];
    if !(path.is_empty() || path.ends_with("::"))
        || !path.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
    {
        return false;
    }
    let Some(arg) = trimmed[at + name.len() + 1..].strip_suffix(");") else {
        return false;
    };
    is_identifier(arg.trim())
}

fn source_line_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for chunk in text.split_inclusive('\n') {
        ranges.push(start..start + chunk.len());
        start += chunk.len();
    }
    if start < text.len() {
        ranges.push(start..text.len());
    }
    ranges
}

fn line_indent<'a>(text: &'a str, range: &Range<usize>) -> &'a str {
    let line = &text[range.clone()];
    &line[..line.len() - line.trim_start().len()]
}

fn frame_literal(line: &str) -> Option<f32> {
    let start = line.find("frame(")? + "frame(".len();
    let comma = line[start..].find(',')? + start;
    let end = line[comma + 1..].find(')')? + comma + 1;
    line[comma + 1..end].trim().parse().ok()
}

pub(crate) fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        // A brace inside a comment, a string or a raw string is not structure:
        // `// if is_excute(agent) {` left in a body used to swallow the real block after it.
        if let Some(next) = skip_rust_literal_or_comment(text, i) {
            i = next;
            continue;
        }
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
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

/// Skip Rust comments and literals while finding lexical block boundaries. In particular,
/// braces inside raw strings must not change the nesting depth used by source-site ownership.
pub(crate) fn skip_rust_literal_or_comment(text: &str, i: usize) -> Option<usize> {
    let rest = text.get(i..)?;
    if rest.starts_with("//") {
        return skip_trivia(text, i);
    }
    if rest.starts_with("/*") {
        return skip_trivia(text, i);
    }

    let bytes = rest.as_bytes();
    // Raw strings have zero or more `#` markers between `r` and the opening quote.
    let raw_prefix = if bytes.first() == Some(&b'r') {
        Some(1)
    } else if bytes.starts_with(b"br") {
        Some(2)
    } else {
        None
    };
    if let Some(prefix) = raw_prefix {
        let hashes = bytes[prefix..]
            .iter()
            .take_while(|byte| **byte == b'#')
            .count();
        let quote = prefix + hashes;
        if bytes.get(quote) == Some(&b'\"') {
            let terminator = format!("\"{}", "#".repeat(hashes));
            let after_open = i + quote + 1;
            let end = text[after_open..]
                .find(&terminator)
                .map(|offset| after_open + offset + terminator.len())
                .unwrap_or(text.len());
            return Some(end);
        }
    }

    let quote_at = if bytes.starts_with(b"br\"") || bytes.starts_with(b"rb\"") {
        Some(2)
    } else if bytes.starts_with(b"b\"") {
        Some(1)
    } else {
        None
    };
    if let Some(quote_at) = quote_at {
        return Some(i + quote_at + string_literal_len(&rest[quote_at..]));
    }

    if rest.starts_with('"') {
        return Some(i + string_literal_len(rest));
    }
    if rest.starts_with('\'') {
        return char_literal_len(rest).map(|len| i + len);
    }
    None
}

fn remove_reverse_line(text: &str, site: &MacroSite) -> Replacement {
    let ranges = source_line_ranges(text);
    let Some(line) = ranges
        .iter()
        .find(|range| range.start <= site.span.start && site.span.start < range.end)
    else {
        return Replacement {
            span: site.span.clone(),
            value: String::new(),
        };
    };
    let trimmed = text[line.clone()].trim();
    if is_lone_agent_call(trimmed, "REVERSE_LR") {
        Replacement {
            span: line.clone(),
            value: String::new(),
        }
    } else {
        let mut end = site.span.end;
        if text[end..].starts_with(';') {
            end += 1;
        }
        Replacement {
            span: site.span.start..end,
            value: String::new(),
        }
    }
}

fn insert_reverse_line(text: &mut String, frame: u32) -> bool {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let ranges = source_line_ranges(text);
    let frame_index = ranges.iter().position(|range| {
        frame_literal(&text[range.clone()])
            .is_some_and(|value| value.round().max(1.0) as u32 == frame)
    });

    // Prefer the existing frame's execute block. This keeps the source's frame ordering and
    // indentation and adds only the one call the user requested.
    if let Some(frame_index) = frame_index {
        let frame_range = ranges[frame_index].clone();
        for range in ranges.iter().skip(frame_index + 1) {
            if frame_literal(&text[range.clone()]).is_some() {
                break;
            }
            let line = &text[range.clone()];
            if opens_is_excute(line) {
                let open = range.start + line.find('{').unwrap();
                if let Some(close) = matching_brace(text, open) {
                    let body_indent = ranges
                        .iter()
                        .skip(frame_index + 1)
                        .find(|body| {
                            body.start > open && body.start < close && {
                                let body_text = &text[body.start..body.end];
                                !body_text.trim().is_empty() && !body_text.trim().starts_with('}')
                            }
                        })
                        .map(|body| line_indent(text, body).to_string())
                        .unwrap_or_else(|| format!("{}    ", line_indent(text, range)));
                    insert_styled(text,
                        close,
                        &format!("{body_indent}macros::REVERSE_LR(agent);{newline}"),
                    );
                    return true;
                }
            }
        }

        // The frame exists but has no execute block. Put a new one directly after the frame
        // line, before another top-level statement or the next frame.
        let indent = line_indent(text, &frame_range);
        let block = format!(
            "{indent}if macros::is_excute(agent) {{{newline}{indent}    macros::REVERSE_LR(agent);{newline}{indent}}}{newline}"
        );
        insert_styled(text, frame_range.end, &block);
        return true;
    }

    // No exact frame exists. Insert a new frame/block before the first later frame, or just
    // before the function's final brace when this is the last point in the move.
    let insert_at = ranges
        .iter()
        .find(|range| {
            frame_literal(&text[range.start..range.end]).is_some_and(|value| value > frame as f32)
        })
        .map(|range| range.start)
        .or_else(|| text.rfind('}'))
        .unwrap_or(text.len());
    let indent = ranges
        .iter()
        .find(|range| frame_literal(&text[range.start..range.end]).is_some())
        .map(|range| line_indent(text, range).to_string())
        .unwrap_or_else(|| "    ".into());
    let block = format!(
        "{indent}frame(agent.lua_state_agent, {frame}.0);{newline}{indent}if macros::is_excute(agent) {{{newline}{indent}    macros::REVERSE_LR(agent);{newline}{indent}}}{newline}"
    );
    insert_styled(text, insert_at, &block);
    true
}

/// Insert one targeted hurtbox state into a flat source frame block. This is deliberately the
/// same conservative shape as `insert_reverse_line`: existing comments/branches stay untouched,
/// while a new top-level frame gets a small execute block that every smashline project accepts.
fn insert_hurt_line(
    text: &mut String,
    frame: u32,
    target: &crate::data::HurtTarget,
    status: &str,
) -> bool {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let call = match target {
        crate::data::HurtTarget::Bone(bone) => format!(
            "macros::HIT_NODE(agent, {}, {});",
            crate::acmd::hash_arg(&bone.to_ascii_lowercase()),
            crate::acmd::emit_status(status)
        ),
        crate::data::HurtTarget::Group(group) => format!(
            "macros::HIT_NO(agent, {group}, {});",
            crate::acmd::emit_status(status)
        ),
        crate::data::HurtTarget::Whole => format!(
            "macros::WHOLE_HIT(agent, {});",
            crate::acmd::emit_status(status)
        ),
    };
    let ranges = source_line_ranges(text);
    let frame_index = ranges.iter().position(|range| {
        frame_literal(&text[range.clone()])
            .is_some_and(|value| value.round().max(1.0) as u32 == frame)
    });

    if let Some(frame_index) = frame_index {
        let frame_range = ranges[frame_index].clone();
        for range in ranges.iter().skip(frame_index + 1) {
            if frame_literal(&text[range.clone()]).is_some() {
                break;
            }
            let line = &text[range.clone()];
            if opens_is_excute(line) {
                let open = range.start + line.find('{').unwrap();
                if let Some(close) = matching_brace(text, open) {
                    let body_indent = ranges
                        .iter()
                        .skip(frame_index + 1)
                        .find(|body| {
                            body.start > open && body.start < close && {
                                let body_text = &text[body.start..body.end];
                                !body_text.trim().is_empty() && !body_text.trim().starts_with('}')
                            }
                        })
                        .map(|body| line_indent(text, body).to_string())
                        .unwrap_or_else(|| format!("{}    ", line_indent(text, range)));
                    insert_styled(text, close, &format!("{body_indent}{call}{newline}"));
                    return true;
                }
            }
        }
        let indent = line_indent(text, &frame_range);
        let block = format!(
            "{indent}if macros::is_excute(agent) {{{newline}{indent}    {call}{newline}{indent}}}{newline}"
        );
        insert_styled(text, frame_range.end, &block);
        return true;
    }

    let insert_at = ranges
        .iter()
        .find(|range| {
            frame_literal(&text[range.start..range.end]).is_some_and(|value| value > frame as f32)
        })
        .map(|range| range.start)
        .or_else(|| text.rfind('}'))
        .unwrap_or(text.len());
    let indent = ranges
        .iter()
        .find(|range| frame_literal(&text[range.start..range.end]).is_some())
        .map(|range| line_indent(text, range).to_string())
        .unwrap_or_else(|| "    ".into());
    let block = format!(
        "{indent}frame(agent.lua_state_agent, {frame}.0);{newline}{indent}if macros::is_excute(agent) {{{newline}{indent}    {call}{newline}{indent}}}{newline}"
    );
    insert_styled(text, insert_at, &block);
    true
}

fn insert_damage_no_reaction_line(
    text: &mut String,
    frame: u32,
    condition: &crate::data::HurtboxCondition,
) -> bool {
    let (mode, value) = match condition {
        crate::data::HurtboxCondition::Normal => (
            "*DAMAGE_NO_REACTION_MODE_NORMAL".to_string(),
            "0".to_string(),
        ),
        crate::data::HurtboxCondition::SuperArmor => (
            "*DAMAGE_NO_REACTION_MODE_ALWAYS".to_string(),
            "0".to_string(),
        ),
        crate::data::HurtboxCondition::ReactionValueArmor { threshold } => (
            "*DAMAGE_NO_REACTION_MODE_REACTION_VALUE".to_string(),
            threshold.to_string(),
        ),
        crate::data::HurtboxCondition::DamageBasedArmor { threshold } => (
            "*DAMAGE_NO_REACTION_MODE_DAMAGE_POWER".to_string(),
            threshold.to_string(),
        ),
        crate::data::HurtboxCondition::DamagePowerCount { threshold } => (
            "*DAMAGE_NO_REACTION_MODE_DAMAGE_POWER_COUNT".to_string(),
            threshold.to_string(),
        ),
        crate::data::HurtboxCondition::NoReactionMode => {
            ("*DAMAGE_NO_REACTION_MODE_NONE".to_string(), "0".to_string())
        }
        crate::data::HurtboxCondition::Unknown { mode, value } => (mode.clone(), value.clone()),
    };
    let call = format!("damage!(agent, *MA_MSC_DAMAGE_DAMAGE_NO_REACTION, {mode}, {value});");
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let ranges = source_line_ranges(text);
    let frame_index = ranges.iter().position(|range| {
        frame_literal(&text[range.clone()])
            .is_some_and(|value| value.round().max(1.0) as u32 == frame)
    });
    if let Some(frame_index) = frame_index {
        let frame_range = ranges[frame_index].clone();
        for range in ranges.iter().skip(frame_index + 1) {
            if frame_literal(&text[range.clone()]).is_some() {
                break;
            }
            let line = &text[range.clone()];
            if opens_is_excute(line) {
                let open = range.start + line.find('{').unwrap();
                if let Some(close) = matching_brace(text, open) {
                    let body_indent = ranges
                        .iter()
                        .skip(frame_index + 1)
                        .find(|body| {
                            body.start > open && body.start < close && {
                                let body_text = &text[body.start..body.end];
                                !body_text.trim().is_empty() && !body_text.trim().starts_with('}')
                            }
                        })
                        .map(|body| line_indent(text, body).to_string())
                        .unwrap_or_else(|| format!("{}    ", line_indent(text, range)));
                    insert_styled(text, close, &format!("{body_indent}{call}{newline}"));
                    return true;
                }
            }
        }
        let indent = line_indent(text, &frame_range);
        insert_styled(text,
            frame_range.end,
            &format!(
                "{indent}if macros::is_excute(agent) {{{newline}{indent}    {call}{newline}{indent}}}{newline}"
            ),
        );
        return true;
    }
    let insert_at = ranges
        .iter()
        .find(|range| {
            frame_literal(&text[range.start..range.end]).is_some_and(|value| value > frame as f32)
        })
        .map(|range| range.start)
        .or_else(|| text.rfind('}'))
        .unwrap_or(text.len());
    let indent = ranges
        .iter()
        .find(|range| frame_literal(&text[range.start..range.end]).is_some())
        .map(|range| line_indent(text, range).to_string())
        .unwrap_or_else(|| "    ".into());
    insert_styled(text,
        insert_at,
        &format!(
            "{indent}frame(agent.lua_state_agent, {frame}.0);{newline}{indent}if macros::is_excute(agent) {{{newline}{indent}    {call}{newline}{indent}}}{newline}"
        ),
    );
    true
}

/// Ensure the `visionary_last_effect_set_*` helpers an added spawn needs exist.
///
/// Generated projects emit these as nested functions inside the effect function
/// that uses them. A user's own source may not have them yet, and inserting a
/// `LAST_EFFECT_SET_WORK_INT` / dynamic `LAST_EFFECT_SET_SCALE_W` line without
/// its helper would ship code that does not build. Insert the missing helper
/// directly inside the effect function, the same place the export puts it.
fn ensure_effect_helpers(text: &mut String, call: &crate::data::EffectCall) {
    if call.work_int.is_some() && !text.contains("fn visionary_last_effect_set_work_int(") {
        let helper = crate::acmd::emit_last_effect_set_work_int_helper("    ").join("\n") + "\n";
        if let Some(open) = text.find('{') {
            // After the opening brace's own line, so the helper starts on its own line.
            let mut at = open + 1;
            if let Some(nl) = text[at..].find('\n') {
                at += nl + 1;
            }
            text.insert_str(at, &helper);
        }
    }
    if call.scale_w.is_some() && !text.contains("fn visionary_last_effect_set_scale_w(") {
        let helper = crate::acmd::emit_last_effect_set_scale_w_helper("    ").join("\n") + "\n";
        if let Some(open) = text.find('{') {
            let mut at = open + 1;
            if let Some(nl) = text[at..].find('\n') {
                at += nl + 1;
            }
            text.insert_str(at, &helper);
        }
    }
}

/// Insert added effect-spawn lines into a flat source frame block.
///
/// Same conservative shape as [`insert_hurt_line`]: prefer the existing frame's
/// `is_excute` block, create one when the frame exists without it, and otherwise
/// insert a new frame block before the first later frame (or the function's
/// final brace). A costume guard wraps the lines rather than replacing the
/// block, so a transplanted recolour stays specific to its costume.
fn insert_effect_lines(
    text: &mut String,
    frame: u32,
    lines: &[String],
    guard: Option<&str>,
) -> bool {
    if lines.is_empty() {
        return true;
    }
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    // Render the payload once, so the three insertion sites below cannot drift apart.
    let render_into = |body_indent: &str| -> String {
        let mut out = String::new();
        match guard {
            Some(header) => {
                out.push_str(&format!("{body_indent}{header}{newline}"));
                for line in lines {
                    out.push_str(&format!("{body_indent}    {line}{newline}"));
                }
                out.push_str(&format!("{body_indent}}}{newline}"));
            }
            None => {
                for line in lines {
                    out.push_str(&format!("{body_indent}{line}{newline}"));
                }
            }
        }
        out
    };
    let ranges = source_line_ranges(text);
    let frame_index = ranges.iter().position(|range| {
        frame_literal(&text[range.clone()])
            .is_some_and(|value| value.round().max(1.0) as u32 == frame)
    });

    if let Some(frame_index) = frame_index {
        let frame_range = ranges[frame_index].clone();
        for range in ranges.iter().skip(frame_index + 1) {
            if frame_literal(&text[range.clone()]).is_some() {
                break;
            }
            let line = &text[range.clone()];
            if opens_is_excute(line) {
                let open = range.start + line.find('{').unwrap();
                if let Some(close) = matching_brace(text, open) {
                    let body_indent = ranges
                        .iter()
                        .skip(frame_index + 1)
                        .find(|body| {
                            body.start > open && body.start < close && {
                                let body_text = &text[body.start..body.end];
                                !body_text.trim().is_empty() && !body_text.trim().starts_with('}')
                            }
                        })
                        .map(|body| line_indent(text, body).to_string())
                        .unwrap_or_else(|| format!("{}    ", line_indent(text, range)));
                    insert_styled(text, close, &render_into(&body_indent));
                    return true;
                }
            }
        }
        let indent = line_indent(text, &frame_range);
        let body_indent = format!("{indent}    ");
        let block = format!(
            "{indent}if macros::is_excute(agent) {{{newline}{}{indent}}}{newline}",
            render_into(&body_indent)
        );
        insert_styled(text, frame_range.end, &block);
        return true;
    }

    let insert_at = ranges
        .iter()
        .find(|range| {
            frame_literal(&text[range.start..range.end]).is_some_and(|value| value > frame as f32)
        })
        .map(|range| range.start)
        .or_else(|| text.rfind('}'))
        .unwrap_or(text.len());
    let indent = ranges
        .iter()
        .find(|range| frame_literal(&text[range.start..range.end]).is_some())
        .map(|range| line_indent(text, range).to_string())
        .unwrap_or_else(|| "    ".into());
    let body_indent = format!("{indent}    ");
    let block = format!(
        "{indent}frame(agent.lua_state_agent, {frame}.0);{newline}{indent}if macros::is_excute(agent) {{{newline}{}{indent}}}{newline}",
        render_into(&body_indent)
    );
    insert_styled(text, insert_at, &block);
    true
}

/// Rewrite `REVERSE_LR` presence and frame placement in a user's own `game_` source.
///
/// This is the one structural source rewrite in the first E1 slice. It is deliberately limited
/// to flat, one-call-per-source-site functions: inserting into a runtime branch or a loop would
/// require choosing an execution context the editor does not know. Such a change is reported
/// without touching the user's text. An LCS over the one-based frame sequence makes a retime a
/// safe remove-plus-insert and keeps later calls from being shifted by an earlier removal.
pub fn rewrite_reverse_lr(
    text: &str,
    label: &str,
    pristine: &[crate::data::ReverseLrEvent],
    edited: &[crate::data::ReverseLrEvent],
) -> Result<(String, SyncReport)> {
    let sites = reverse_lr_sites(text);
    let parsed = crate::acmd::parse_acmd_script(text);
    fn contains_loop(stmts: &[crate::data::AcmdStmt]) -> bool {
        stmts.iter().any(|stmt| match stmt {
            crate::data::AcmdStmt::Loop { .. } => true,
            crate::data::AcmdStmt::RawBlock { body, .. } => contains_loop(body),
            _ => false,
        })
    }
    let mut report = SyncReport::default();
    let flat_sites = pristine.len() == sites.len()
        && pristine
            .iter()
            .enumerate()
            .all(|(index, event)| event.site == index);
    let flat_edited = edited
        .iter()
        .enumerate()
        .all(|(index, event)| event.site == index);
    let old: Vec<u32> = pristine.iter().map(|event| event.frame).collect();
    let new: Vec<u32> = edited.iter().map(|event| event.frame).collect();
    let has_unsupported_structure =
        parsed.branch_count() > 0 || contains_loop(&parsed.stmts) || !flat_sites || !flat_edited;
    if has_unsupported_structure {
        if pristine != edited {
            report.skipped.push(format!(
                "{label}: REVERSE_LR placement changed inside a loop/branch or after a source-site mismatch — source syncing only edits flat point calls"
            ));
        }
        return Ok((text.to_string(), report));
    }

    let n = old.len();
    let m = new.len();
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if old[i] == new[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut matched_old = vec![false; n];
    let mut matched_new = vec![false; m];
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if old[i] == new[j] {
            matched_old[i] = true;
            matched_new[j] = true;
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    if matched_old.iter().all(|matched| *matched) && matched_new.iter().all(|matched| *matched) {
        return Ok((text.to_string(), report));
    }

    let removals: Vec<Replacement> = sites
        .iter()
        .enumerate()
        .filter(|(index, _)| !matched_old[*index])
        .map(|(_, site)| remove_reverse_line(text, site))
        .collect();
    let mut updated = apply(text, removals);
    for (_, event) in edited
        .iter()
        .enumerate()
        .filter(|(index, _)| !matched_new[*index])
    {
        if insert_reverse_line(&mut updated, event.frame) {
            report.changed += 1;
        }
    }
    report.changed += matched_old.iter().filter(|matched| !**matched).count();
    Ok((updated, report))
}

// ── Expression write-back ───────────────────────────────────────────────────

/// The measured expression calls in source order, with the leading `agent` included in the
/// arity just as [`MacroSite::args`] stores it.
pub(crate) fn expression_sites(text: &str) -> Vec<MacroSite> {
    const CALLS: &[(&str, usize)] = &[
        ("RUMBLE_HIT", 3),
        ("QUAKE", 2),
        ("FT_ATTACK_ABS_CAMERA_QUAKE", 3),
    ];
    let mut sites: Vec<MacroSite> = scan_macro_sites(text, 0..text.len())
        .into_iter()
        .filter(|site| {
            CALLS
                .iter()
                .any(|(name, arity)| site.name == *name && site.args.len() == *arity)
                // A project helper around `set_rumble` (see `crate::project_helpers`).
                || (site.name == "ControlModule::set_rumble" && site.args.len() == 5)
        })
        .collect();
    sites.extend(
        scan_named_sites(text, "ControlModule::set_rumble", 0..text.len())
            .into_iter()
            .filter(|site| site.args.len() == 5),
    );
    sites.sort_by_key(|site| site.span.start);
    sites
}

fn expression_tokens(call: &crate::data::ExpressionCall) -> Vec<&str> {
    match call {
        crate::data::ExpressionCall::RumbleHit { kind, unk } => vec![kind, unk],
        crate::data::ExpressionCall::Quake { kind } => vec![kind],
        crate::data::ExpressionCall::FtAttackAbsCameraQuake {
            attack_abs_kind,
            quake_kind,
        } => vec![attack_abs_kind, quake_kind],
        crate::data::ExpressionCall::ControlModuleSetRumble {
            kind,
            duration,
            looped,
            target,
            ..
        } => vec![kind, duration, looped, target],
    }
}

/// Rewrite the argument tokens of the measured expression calls in the user's own source.
///
/// This is intentionally token-based rather than numeric. Camera/rumble constants are not one
/// collision namespace: a mod can use a named constant, a raw capture value, or a project-local
/// expression, and replacing any of those with a guessed `Hash40`/integer would change the
/// caller's intent. Frames, macro names, and call count are structural and are reported rather
/// than moved or invented. A looped call is one source line, so all unrolled events from that
/// site must agree on the replacement.
pub fn rewrite_expressions(
    text: &str,
    label: &str,
    pristine: &[crate::data::ExpressionEvent],
    edited: &[crate::data::ExpressionEvent],
) -> Result<(String, SyncReport)> {
    let sites = expression_sites(text);
    let mut report = SyncReport::default();
    if pristine.len() != edited.len() {
        report.skipped.push(format!(
            "{label}: {} expression call(s) were added or removed — source syncing only retunes existing calls",
            edited.len().abs_diff(pristine.len())
        ));
        return Ok((text.to_string(), report));
    }

    let mut edits: Vec<Replacement> = Vec::new();
    for (before, now) in pristine.iter().zip(edited.iter()) {
        if before == now {
            continue;
        }
        let Some(site) = sites.get(before.site) else {
            report.skipped.push(format!(
                "{label}: expression call #{} has no matching source call",
                before.site
            ));
            continue;
        };
        if before.site != now.site || before.frame != now.frame {
            report.skipped.push(format!(
                "{label}: `{}` on frame {} was moved or reordered — source syncing cannot change expression structure",
                before.call.func(),
                before.frame
            ));
            continue;
        }
        if before.call.func() != now.call.func() || site.name != before.call.func() {
            report.skipped.push(format!(
                "{label}: expression call #{} changed macro — source syncing only rewrites its argument values",
                before.site
            ));
            continue;
        }
        let old_tokens = expression_tokens(&before.call);
        let new_tokens = expression_tokens(&now.call);
        if old_tokens.len() != new_tokens.len() || site.args.len() != old_tokens.len() + 1 {
            report.skipped.push(format!(
                "{label}: `{}` on frame {} has an unsupported argument shape",
                before.call.func(),
                before.frame
            ));
            continue;
        }
        for (index, (old, new)) in old_tokens.iter().zip(new_tokens).enumerate() {
            if *old == new {
                continue;
            }
            let Some(span) = site.args.get(index + 1) else {
                continue;
            };
            let Some(edit) = text_edit(text, span, new) else {
                continue;
            };
            match edits.iter().find(|existing| existing.span == edit.span) {
                Some(existing) if existing.value == edit.value => {}
                Some(_) => report.skipped.push(format!(
                    "{label}: looped `{}` on frame {} was given conflicting values in one source line",
                    before.call.func(),
                    before.frame
                )),
                None => edits.push(edit),
            }
        }
    }

    report.changed = edits.len();
    Ok((apply(text, edits), report))
}

// ── Sound write-back ─────────────────────────────────────────────────────────

/// The sound calls in `text`, in document order — index `n` is site `n`.
///
/// The arity filter is load-bearing for the same reason it is in [`hurt_sites`]: the parser
/// refuses a family member written with the wrong number of arguments and leaves it `Raw`, so
/// counting it here would number every later site one too high and retune the wrong call.
///
/// `SOUND_FUNCS` carries `(name, hash args, has tail)`, and the total after `agent` is the sum
/// of the last two.
#[cfg(test)]
pub(crate) fn sound_sites(text: &str) -> Vec<MacroSite> {
    sound_sites_with(text, &crate::hash_macros::current())
}

/// [`sound_sites`] reading hash macros from `macros` — the linked project's own table.
pub(crate) fn sound_sites_with(
    text: &str,
    macros: &crate::hash_macros::HashMacros,
) -> Vec<MacroSite> {
    scan_macro_sites(text, 0..text.len())
        .into_iter()
        .filter(|site| {
            crate::acmd::SOUND_FUNCS.iter().any(|(name, hashes, tail)| {
                site.name == *name
                    && site.args.len() == hashes + usize::from(*tail) + 1
                    // The parser types a call only when every sound is a name it can read; a
                    // `PLAY_SE(agent, sound)` with a variable stays raw. Counting it here anyway
                    // numbered every later site one too high.
                    && site.args[1..=*hashes].iter().all(|span| {
                        let arg = text[span.clone()].trim();
                        (arg.starts_with("Hash40::new(\"") && arg.ends_with("\")"))
                            || macros.expand_call(arg).is_some()
                    })
            })
        })
        .collect()
}

/// Rewrite edited sound names into the user's own source.
///
/// Value edits only, as everywhere on this path — and for sound the value is the `Hash40` naming
/// which sound plays. A retime is reported rather than performed, for the reason
/// [`rewrite_hurtboxes`] gives: the frame is the block the call sits in, not an argument. So is
/// a change of macro, which is a different call taking different arguments rather than a
/// different value in the same one.
pub fn rewrite_sounds(
    text: &str,
    label: &str,
    pristine: &[crate::data::SoundEvent],
    edited: &[crate::data::SoundEvent],
) -> Result<(String, SyncReport)> {
    rewrite_sounds_with(text, label, pristine, edited, &crate::hash_macros::current())
}

/// [`rewrite_sounds`] with the project's hash macros given explicitly, so an edited name is
/// written back in the author's own spelling (`se!("jump02")`, not `Hash40::new(…)`).
pub fn rewrite_sounds_with(
    text: &str,
    label: &str,
    pristine: &[crate::data::SoundEvent],
    edited: &[crate::data::SoundEvent],
    macros: &crate::hash_macros::HashMacros,
) -> Result<(String, SyncReport)> {
    let sites = sound_sites_with(text, macros);
    let mut report = SyncReport::default();
    let mut edits: Vec<Replacement> = Vec::new();

    if pristine.len() != edited.len() {
        report.skipped.push(format!(
            "{label}: {} sound call(s) were added or removed — source syncing only rewrites existing sound names",
            edited.len().abs_diff(pristine.len())
        ));
        return Ok((text.to_string(), report));
    }

    if pristine
        .iter()
        .zip(edited)
        .any(|(before, now)| before.site != now.site)
    {
        report.skipped.push(format!(
            "{label}: sound call order changed — source syncing does not move or reorder sound statements"
        ));
        return Ok((text.to_string(), report));
    }

    for (before, now) in pristine.iter().zip(edited.iter()) {
        if before == now {
            continue;
        }
        let Some(site) = sites.get(now.site) else {
            report.skipped.push(format!(
                "{label}: a sound has no matching call in the source — it was added in the \
                 editor, and source syncing only retunes existing calls"
            ));
            continue;
        };
        if before.frame != now.frame {
            report.skipped.push(format!(
                "{label}: the `{}` on frame {} was retimed — its frame is the block it sits in, \
                 not an argument, so source syncing cannot move it",
                site.name, before.frame
            ));
            continue;
        }
        if before.call.func != now.call.func || before.call.sounds.len() != now.call.sounds.len() {
            report.skipped.push(format!(
                "{label}: the `{}` on frame {} became a `{}` — a different macro takes different \
                 arguments, so that is structure rather than a value",
                before.call.func, before.frame, now.call.func
            ));
            continue;
        }
        if before.call.tail != now.call.tail {
            report.skipped.push(format!(
                "{label}: the suppression window on the `{}` at frame {} changed — only the \
                 sound name is editable here",
                before.call.func, before.frame
            ));
        }
        for (index, (was, is)) in before
            .call
            .sounds
            .iter()
            .zip(now.call.sounds.iter())
            .enumerate()
        {
            if was == is {
                continue;
            }
            // `+ 1` to step over `agent`, which every one of these macros takes first.
            let Some(span) = site.args.get(index + 1) else {
                continue;
            };
            // In the author's own spelling: `se!("jump02")` stays a `se!`.
            let spelled = macros.spell(&text[span.clone()], is, text);
            let Some(edit) = text_edit(text, span, &spelled) else {
                continue;
            };
            // One looped `PLAY_SE` yields one event per iteration, all carrying the same site,
            // so the same span can be reached more than once. Writing it twice would hand
            // `apply` two overlapping edits; a *disagreement* between the two is a real conflict
            // and is reported rather than resolved by whichever iteration happened to be last.
            match edits.iter().find(|e| e.span == edit.span) {
                Some(existing) if existing.value == edit.value => {}
                Some(_) => report.skipped.push(format!(
                    "{label}: the looped `{}` at frame {} was given two different sounds in the \
                     same line — every iteration is the one call in the file",
                    before.call.func, before.frame
                )),
                None => edits.push(edit),
            }
        }
    }

    report.changed = edits.len();
    Ok((apply(text, edits), report))
}

/// Write edited sound names back into the project's own `sound_` function.
pub fn sync_sounds(
    index: &SourceIndex,
    fighter: &str,
    move_name: &str,
    pristine: &[crate::data::SoundEvent],
    edited: &[crate::data::SoundEvent],
) -> Result<SyncReport> {
    let script_name = crate::acmd::acmd_script_name("sound", move_name);
    sync_script(index, fighter, &script_name, |body| {
        rewrite_sounds_with(
            body,
            &format!("{fighter}/{move_name}"),
            pristine,
            edited,
            &index.hash_macros,
        )
    })
}

/// Sync edited hurtbox state for one move back into the project source on disk.
pub fn sync_hurtboxes(
    index: &SourceIndex,
    fighter: &str,
    move_name: &str,
    pristine: &(
        Vec<crate::data::HurtboxState>,
        Vec<crate::data::ColPriState>,
    ),
    edited: &(
        Vec<crate::data::HurtboxState>,
        Vec<crate::data::ColPriState>,
    ),
) -> Result<SyncReport> {
    let script_name = crate::acmd::acmd_script_name("game", move_name);
    sync_script(index, fighter, &script_name, |body| {
        rewrite_hurtboxes(body, &format!("{fighter}/{move_name}"), pristine, edited)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kirby's down smash, verbatim from the corpus — two rate calls around a `frame()`.
    const DOWN_SMASH: &str = "unsafe extern \"C\" fn game_attacklw4(agent: &mut L2CAgentBase) {\n\
        \x20   macros::FT_MOTION_RATE(agent, 0.25);\n\
        \x20   frame(agent.lua_state_agent, 4.0);\n\
        \x20   macros::FT_MOTION_RATE(agent, 1.0);\n\
        }\n";

    /// A rate edit rewrites **only** that argument's span, leaving the rest byte for byte.
    ///
    /// The paired positive matters as much as the change: a rewriter that returned the whole
    /// file re-rendered from the model would pass "the new value is present" while quietly
    /// reformatting the user's own source, which is the thing this path exists to avoid.
    #[test]
    fn a_rate_edit_rewrites_only_that_argument() {
        let edited = crate::acmd::parse_acmd_script(DOWN_SMASH);
        let mut sites = edited.motion_rate_sites();
        assert_eq!(sites.len(), 2, "the fixture has two rate calls");
        sites[0].2 = 0.5;

        let (out, report) = rewrite_motion_rates(DOWN_SMASH, "kirby/attack_lw4", &sites).unwrap();
        assert_eq!(report.changed, 1, "one argument changed");
        assert!(report.skipped.is_empty(), "{:?}", report.skipped);
        assert_eq!(
            out,
            DOWN_SMASH.replace("agent, 0.25", "agent, 0.5"),
            "only the first rate's argument may differ"
        );
    }

    /// The site-ordinal guard.
    ///
    /// The parser models a rate call only at the function's top level, so one written inside a
    /// runtime branch never reaches the editor — while the source scan still counts it. Writing
    /// by position across that mismatch lands the edit on the *wrong call*, silently, which is
    /// the failure this codebase has already paid for once with per-family site counters.
    ///
    /// Paired with a positive: the same source with the branch removed must still write, or this
    /// test would pass against a rewriter that refuses everything.
    #[test]
    fn a_rate_inside_a_branch_is_refused_rather_than_written_by_position() {
        let branched = "unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {\n\
            \x20   if WorkModule::is_flag(agent.module_accessor, 0) {\n\
            \x20       macros::FT_MOTION_RATE(agent, 0.25);\n\
            \x20   }\n\
            \x20   macros::FT_MOTION_RATE(agent, 1.0);\n\
            }\n";
        let script = crate::acmd::parse_acmd_script(branched);
        let sites = script.motion_rate_sites();
        assert_eq!(sites.len(), 1, "only the top-level call is modelled");

        let (out, report) = rewrite_motion_rates(branched, "kirby/x", &sites).unwrap();
        assert_eq!(
            out, branched,
            "nothing may be written when the counts disagree"
        );
        assert!(
            report.skipped.iter().any(|s| s.contains("branch")),
            "the refusal must say why: {:?}",
            report.skipped
        );

        // The positive half, through the same function.
        let flat = crate::acmd::parse_acmd_script(DOWN_SMASH);
        let (_, ok) =
            rewrite_motion_rates(DOWN_SMASH, "kirby/x", &flat.motion_rate_sites()).unwrap();
        assert!(
            ok.skipped.is_empty(),
            "a flat script must not be refused: {:?}",
            ok.skipped
        );
    }

    #[test]
    fn an_expression_token_edit_rewrites_only_that_argument() {
        let source = r#"unsafe extern "C" fn expression_throwhi(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        // Keep this comment and the unknown expression line byte-for-byte.
        slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE, *SLOPE_STATUS_LR);
        macros::RUMBLE_HIT(agent, Hash40::new("rbkind_attackm"), 0);
        macros::QUAKE(agent, *CAMERA_QUAKE_KIND_M);
    }
}
"#;
        let script = crate::acmd::parse_expression_script(source);
        let pristine = script.to_expression_events();
        let mut edited_script = script.clone();
        let call = edited_script.expression_stmt_mut(0).unwrap();
        *call = crate::data::ExpressionCall::RumbleHit {
            kind: "Hash40::new(\"rbkind_attackl\")".into(),
            unk: "0".into(),
        };
        let edited = edited_script.to_expression_events();

        let (out, report) =
            rewrite_expressions(source, "kirby/throw_hi", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1);
        assert!(report.skipped.is_empty(), "{:?}", report.skipped);
        assert!(out.contains("Hash40::new(\"rbkind_attackl\")"));
        assert!(out.contains("slope!(agent, *MA_MSC_CMD_SLOPE_SLOPE, *SLOPE_STATUS_LR);"));
        assert!(out.contains("// Keep this comment"));
        assert_eq!(
            crate::acmd::parse_expression_script(&out).to_expression_events(),
            edited
        );
    }

    #[test]
    fn a_direct_rumble_edit_rewrites_payload_but_keeps_symbolic_target() {
        let source = r#"unsafe extern "C" fn expression_walk(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::QUAKE(agent, *CAMERA_QUAKE_KIND_M);
        ControlModule::set_rumble(agent.module_accessor, Hash40::new("rbkind_walk"), 0, false, *BATTLE_OBJECT_ID_INVALID as u32);
        ControlModule::set_rumble(agent.module_accessor);
    }
}
"#;
        let script = crate::acmd::parse_expression_script(source);
        let pristine = script.to_expression_events();
        assert_eq!(pristine.len(), 2);
        assert_eq!(pristine[1].call.func(), "ControlModule::set_rumble");

        let mut edited_script = script.clone();
        let call = edited_script.expression_stmt_mut(1).unwrap();
        *call = crate::data::ExpressionCall::ControlModuleSetRumble {
            receiver: "agent.module_accessor".into(),
            kind: "Hash40::new(\"rbkind_attackl\")".into(),
            duration: "6".into(),
            looped: "true".into(),
            target: "*BATTLE_OBJECT_ID_INVALID as u32".into(),
        };
        let edited = edited_script.to_expression_events();

        let (out, report) = rewrite_expressions(source, "kirby/walk", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 3);
        assert!(report.skipped.is_empty(), "{:?}", report.skipped);
        assert!(out.contains(
            "ControlModule::set_rumble(agent.module_accessor, Hash40::new(\"rbkind_attackl\"), 6, true, *BATTLE_OBJECT_ID_INVALID as u32);"
        ));
        assert!(out.contains("ControlModule::set_rumble(agent.module_accessor);"));
        assert_eq!(
            crate::acmd::parse_expression_script(&out).to_expression_events(),
            edited
        );
    }

    #[test]
    fn an_expression_retime_is_reported_without_touching_source() {
        let source = r#"unsafe extern "C" fn expression_x(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::QUAKE(agent, *CAMERA_QUAKE_KIND_S);
    }
}
"#;
        let script = crate::acmd::parse_expression_script(source);
        let pristine = script.to_expression_events();
        let mut edited = pristine.clone();
        edited[0].frame = 5;
        let (out, report) = rewrite_expressions(source, "kirby/x", &pristine, &edited).unwrap();
        assert_eq!(out, source);
        assert_eq!(report.changed, 0);
        assert!(report.skipped.iter().any(|note| note.contains("moved")));
    }

    const REVERSE_FLAT: &str = r#"unsafe extern "C" fn game_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 3.0);
    if macros::is_excute(agent) {
        macros::REVERSE_LR(agent);
    }
    frame(agent.lua_state_agent, 8.0);
    if macros::is_excute(agent) {
        macros::REVERSE_LR(agent);
    }
}
pub fn install(agent: &mut smashline::Agent) {
    agent.acmd("game_attackairn", game_attackairn, smashline::Priority::Default);
}
"#;

    #[test]
    fn reverse_lr_source_write_back_retimes_without_reformatting_the_other_point() {
        let script = crate::acmd::parse_acmd_script(REVERSE_FLAT);
        let pristine = script.to_reverse_lr_events();
        let mut edited = pristine.clone();
        edited[0].frame = 4;

        let (out, report) =
            rewrite_reverse_lr(REVERSE_FLAT, "mario/attack_air_n", &pristine, &edited).unwrap();
        assert_eq!(
            report.changed, 2,
            "one source call was removed and one added"
        );
        assert!(report.skipped.is_empty(), "{:?}", report.skipped);
        assert_eq!(
            crate::acmd::parse_acmd_script(&out).to_reverse_lr_events(),
            edited,
            "the source rewriter and parser must agree on the edited points"
        );
        assert_eq!(
            out.matches("macros::REVERSE_LR(agent);").count(),
            2,
            "retiming must not duplicate the later point"
        );
        assert!(
            out.contains("frame(agent.lua_state_agent, 8.0);"),
            "the untouched frame must remain byte-for-byte present"
        );
    }

    #[test]
    fn reverse_lr_source_write_back_refuses_a_runtime_branch() {
        let branched = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, 0) {
        frame(agent.lua_state_agent, 3.0);
        if macros::is_excute(agent) {
            macros::REVERSE_LR(agent);
        }
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(branched).to_reverse_lr_events();
        let mut edited = pristine.clone();
        edited[0].frame = 4;
        let (out, report) =
            rewrite_reverse_lr(branched, "mario/branch", &pristine, &edited).unwrap();
        assert_eq!(out, branched, "a branch placement must not be guessed at");
        assert!(
            report.skipped.iter().any(|note| note.contains("branch")),
            "the refusal must name the structural blocker: {:?}",
            report.skipped
        );
    }

    #[test]
    fn reverse_lr_source_site_mismatch_is_safe_when_another_family_changes() {
        let mismatch = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 3.0);
    macros::REVERSE_LR(agent);
}
"#;
        // The parser intentionally models only the measured execute-block form, while the
        // source scanner still sees this line. A hitbox-only edit can therefore invoke this
        // pass with equal empty baselines; it must be a no-op, not an ordinal-index panic.
        let events = Vec::new();
        let (out, report) = rewrite_reverse_lr(mismatch, "mario/mismatch", &events, &events)
            .expect("a source-site mismatch is reportable, not fatal");
        assert_eq!(out, mismatch);
        assert!(report.changed == 0 && report.skipped.is_empty());
    }

    #[test]
    fn syncing_reverse_lr_reindexes_the_projected_source_shape() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "src/mario/acmd.rs", REVERSE_FLAT);
        write(
            tmp.path(),
            "src/mario/mod.rs",
            "pub fn install() { let agent = &mut smashline::Agent::new(\"mario\"); }",
        );
        let index = SourceIndex::build(tmp.path()).unwrap();
        let source = index.script_source("mario", "attack_air_n").unwrap().body;
        let pristine = crate::acmd::parse_acmd_script(&source).to_reverse_lr_events();
        let mut edited = pristine.clone();
        edited[0].frame = 4;
        let report = sync_reverse_lr(&index, "mario", "attack_air_n", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 2);
        let after = SourceIndex::build(tmp.path()).unwrap();
        let body = after.script_source("mario", "attack_air_n").unwrap().body;
        assert_eq!(
            crate::acmd::parse_acmd_script(&body).to_reverse_lr_events(),
            edited
        );
    }

    /// `FT_MOTION_RATE_RANGE` must not be counted as a rate site.
    ///
    /// It begins with `FT_MOTION_RATE`'s entire name, so a prefix match would count it, shift
    /// every later site by one, and write a playback rate into its first frame argument.
    #[test]
    fn the_range_macro_is_not_counted_as_a_rate_site() {
        let text = "unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {\n\
            \x20   macros::FT_MOTION_RATE_RANGE(agent, 5.0, 10.0, 0.5);\n\
            \x20   macros::FT_MOTION_RATE(agent, 0.6);\n\
            }\n";
        let sites = motion_rate_sites(text);
        assert_eq!(sites.len(), 1, "only the plain form is a rate site");
        assert_eq!(sites[0].arg(text, 1).map(str::trim), Some("0.6"));
    }

    fn write(root: &Path, rel: &str, body: &str) -> PathBuf {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    const MARIO: &str = r#"
use smash::{lua2cpp::*, phx::*};

unsafe extern "C" fn game_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        // A comment mentioning macros::EFFECT that must not be scanned.
        macros::ATTACK(agent, 0, 0, Hash40::new("top"), 8.0, 361, 100, 0, 40, 4.5, 0.0, 8.0, 6.0, None, None, None, 1.0, 1.0, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_POS, false, 0, 0.0, 0, false, false, false, false, false, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_M, *COLLISION_SOUND_ATTR_PUNCH, *ATTACK_REGION_PUNCH);
    }
}

unsafe extern "C" fn effect_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_FLIP(agent, Hash40::new("sys_hit_l"), Hash40::new("sys_hit_r"), Hash40::new("haver"), 1.0, 2.0, 3.0, 0.0, 90.0, 45.0, 1.5, true, *EF_FLIP_YZ);
    }
}

pub fn install(agent: &mut smashline::Agent) {
    agent.acmd("game_attackairn", game_attackairn, smashline::Priority::Default);
    agent.acmd("effect_attackairn", effect_attackairn, smashline::Priority::Default);
}
"#;

    /// Shy Guy's forward smash: both directions forward to one `_impl` with a facing argument.
    const SHARED_SMASH: &str = r#"
use super::*;

#[derive(Clone, Copy)]
enum SmashFacing { Forward, Reverse }

impl SmashFacing {
    fn z(self) -> f32 { match self { Self::Forward => 1.0, Self::Reverse => -1.0 } }
    #[inline]
    fn lr_check(self) -> i32 {
        unsafe { match self { Self::Forward => *ATTACK_LR_CHECK_F, Self::Reverse => *ATTACK_LR_CHECK_B } }
    }
}

unsafe fn game_attacks4_impl(agent: &mut L2CAgentBase, facing: SmashFacing) {
    // The reverse sweetspot doing 21.5 instead of 21.0 is on purpose
    let (hit_frame, clear_frame, sweet_damage) = match facing {
        SmashFacing::Forward => (21.0, 25.0, 21.0),
        SmashFacing::Reverse => (24.0, 28.0, 21.5),
    };
    frame(agent.lua_state_agent, 3.0);
    if macros::is_excute(agent) {
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD);
    }
    frame(agent.lua_state_agent, hit_frame);
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("top"), sweet_damage, 42, 80, 0, 35, 6.0, 0.0, 11.6, 28.4 * facing.z(), None, None, None, 1.2, 1.0, *ATTACK_SETOFF_KIND_ON, facing.lr_check(), false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_L, *COLLISION_SOUND_ATTR_HEAVY, *ATTACK_REGION_OBJECT);
        macros::ATTACK(agent, 1, 0, Hash40::new("top"), 14.5, 42, 90, 0, 35, 2.4, 0.0, 10.7, 21.6 * facing.z(), Some(0.0), Some(10.0), Some(12.0 * facing.z()), 1.0, 1.0, *ATTACK_SETOFF_KIND_ON, facing.lr_check(), false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_M, *COLLISION_SOUND_ATTR_HEAVY, *ATTACK_REGION_OBJECT);
    }
    frame(agent.lua_state_agent, clear_frame);
    if macros::is_excute(agent) {
        AttackModule::clear_all(agent.module_accessor);
    }
}

unsafe fn effect_attacks4_impl(agent: &mut L2CAgentBase, facing: SmashFacing) {
    let (smoke_frame, impact_frame) = match facing {
        SmashFacing::Forward => (20.0, 21.0),
        SmashFacing::Reverse => (23.0, 24.0),
    };
    frame(agent.lua_state_agent, smoke_frame);
    if macros::is_excute(agent) {
        macros::LANDING_EFFECT(agent, Hash40::new("sys_h_smoke_b"), Hash40::new("top"), 0, 0, -7.0 * facing.z(), 0, 0, 0, 0.7, 0, 0, 0, 0, 0, 0, true);
    }
    frame(agent.lua_state_agent, impact_frame);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_attack_impact"), Hash40::new("top"), 0, 11.6, 28.4 * facing.z(), 90, 0, 0, 1.95, 0, 0, 0, 0, 0, 0, true);
    }
}

unsafe extern "C" fn game_attacks4(agent: &mut L2CAgentBase) { game_attacks4_impl(agent, SmashFacing::Forward) }
unsafe extern "C" fn game_attacks4reverse(agent: &mut L2CAgentBase) { game_attacks4_impl(agent, SmashFacing::Reverse) }
unsafe extern "C" fn effect_attacks4(agent: &mut L2CAgentBase) { effect_attacks4_impl(agent, SmashFacing::Forward) }
unsafe extern "C" fn effect_attacks4reverse(agent: &mut L2CAgentBase) { effect_attacks4_impl(agent, SmashFacing::Reverse) }

pub(super) fn install(agent: &mut Agent) {
    agent
        .game_acmd("game_attacks4", game_attacks4, Priority::Default)
        .effect_acmd("effect_attacks4", effect_attacks4, Priority::Default)
        .game_acmd("game_attacks4reverse", game_attacks4reverse, Priority::Default)
        .effect_acmd("effect_attacks4reverse", effect_attacks4reverse, Priority::Default)
        ;
}
"#;

    /// The Effects page reads which moves play a moveset's own effects from these bodies:
    /// the fighter's scripts and its objects', and nobody else's.
    #[test]
    fn script_bodies_cover_the_fighter_and_its_objects_only() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "src/pacman/acmd.rs",
            r#"unsafe extern "C" fn effect_attack_lw3(agent: &mut L2CAgentBase) {
    EFFECT(agent, Hash40::new("carrot_pull"), Hash40::new("top"), 0, 0, 6.8, 0, 0, 0, 0.8, 0, 0, 0, 0, 0, 0, true);
}
unsafe extern "C" fn effect_hop(agent: &mut L2CAgentBase) {
    macros::EFFECT_FOLLOW(agent, Hash40::new("fryguy_fire"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 4.4, true);
}
unsafe extern "C" fn effect_win(agent: &mut L2CAgentBase) {
    macros::EFFECT(agent, Hash40::new("mario_only"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, true);
}
pub fn install() {
    Agent::new("pacman").acmd("effect_attacklw3", effect_attack_lw3, Priority::Low).install();
    Agent::new("pacman_bigpacman").acmd("effect_specialnhop", effect_hop, Priority::Low).install();
    Agent::new("mario").acmd("effect_win1", effect_win, Priority::Low).install();
}
"#,
        );
        let index = SourceIndex::build(tmp.path()).unwrap();
        let bodies = index.fighter_script_bodies("pacman");
        let found: Vec<(&str, &str)> = bodies
            .iter()
            .map(|(agent, script, _)| (agent.as_str(), script.as_str()))
            .collect();
        assert_eq!(
            found,
            vec![
                ("pacman", "effect_attacklw3"),
                ("pacman_bigpacman", "effect_specialnhop")
            ]
        );
        assert!(bodies[0].2.contains("carrot_pull"), "{}", bodies[0].2);
        assert!(bodies[1].2.contains("fryguy_fire"), "{}", bodies[1].2);
    }

    fn shared_smash_project(acmd: &str) -> (tempfile::TempDir, SourceIndex) {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "src/pacman/acmd/smashes.rs", acmd);
        write(
            tmp.path(),
            "src/pacman/mod.rs",
            "pub fn install() { let agent = &mut smashline::Agent::new(\"pacman\"); }",
        );
        let index = SourceIndex::build(tmp.path()).unwrap();
        (tmp, index)
    }

    #[test]
    fn forwarding_scripts_read_the_shared_body_resolved_for_each_caller() {
        let (_tmp, index) = shared_smash_project(SHARED_SMASH);
        let reverse = index.script_source("pacman", "attacks4reverse").unwrap();
        assert!(reverse.notes.is_empty(), "{:?}", reverse.notes);
        assert_eq!(reverse.covers, vec!["game_", "effect_"]);
        let body = &reverse.body;
        assert!(body.contains("fn game_attacks4reverse("), "{body}");
        assert!(!body.contains("facing"), "every facing read resolves: {body}");
        assert!(!body.contains("let ("), "the resolved binding is dropped: {body}");
        assert!(body.contains("frame(agent.lua_state_agent, 24.0);"), "{body}");
        assert!(body.contains("frame(agent.lua_state_agent, 28.0);"), "{body}");
        assert!(body.contains("Some(-12.0)"), "{body}");
        assert!(body.contains("*ATTACK_LR_CHECK_B"), "{body}");
        // `-7.0 * -1.0`: the leading minus is the operand's sign.
        assert!(body.contains("0, 0, 7.0, 0"), "{body}");

        let hitboxes = crate::acmd::parse_acmd_script(body).to_hitboxes();
        assert_eq!(hitboxes.len(), 2, "{body}");
        assert_eq!(hitboxes[0].damage, 21.5);
        assert_eq!(hitboxes[0].offset_z, -28.4);
        assert_eq!(hitboxes[0].active_start, 24);
        assert_eq!(hitboxes[1].offset_z, -21.6);
        assert_eq!(hitboxes[1].capsule_end.map(|end| end[2]), Some(-12.0));

        let forward = index.script_source("pacman", "attacks4").unwrap();
        let hitboxes = crate::acmd::parse_acmd_script(&forward.body).to_hitboxes();
        assert_eq!(hitboxes.len(), 2, "{}", forward.body);
        assert_eq!(hitboxes[0].damage, 21.0);
        assert_eq!(hitboxes[0].offset_z, 28.4);
        assert_eq!(hitboxes[0].active_start, 21);
        assert!(forward.body.contains("*ATTACK_LR_CHECK_F"), "{}", forward.body);
    }

    #[test]
    fn a_shared_body_refuses_write_back_instead_of_changing_every_variant() {
        let (tmp, index) = shared_smash_project(SHARED_SMASH);
        let site = index.script("pacman", "game_attacks4reverse").unwrap();
        let delegate = site.delegate.as_ref().expect("the wrapper forwards");
        assert_eq!(delegate.function, "game_attacks4_impl");
        assert_eq!(delegate.callers, 2);
        assert_eq!(site.function, "game_attacks4reverse", "the wrapper keeps its own site");

        let body = index.script_source("pacman", "attacks4reverse").unwrap().body;
        let pristine = crate::acmd::parse_acmd_script(&body).to_hitboxes();
        let mut edited = pristine.clone();
        edited[0].damage = 30.0;
        let error = sync_hitboxes(&index, "pacman", "attacks4reverse", &pristine, &edited)
            .expect_err("one variant's edit must not reach the shared body");
        assert!(error.to_string().contains("2 scripts share"), "{error}");
        let on_disk =
            std::fs::read_to_string(tmp.path().join("src/pacman/acmd/smashes.rs")).unwrap();
        assert_eq!(on_disk, SHARED_SMASH);
    }

    #[test]
    fn values_that_cannot_be_resolved_are_reported_not_guessed() {
        let acmd = SHARED_SMASH
            .replace(
                "frame(agent.lua_state_agent, 3.0);",
                "frame(agent.lua_state_agent, facing.charge_frame());",
            )
            .replace(
                "game_attacks4_impl(agent, SmashFacing::Reverse)",
                "game_attacks4_impl(agent, pick_facing(agent))",
            );
        let (_tmp, index) = shared_smash_project(&acmd);

        let forward = index.script_source("pacman", "attacks4").unwrap();
        assert!(
            forward
                .notes
                .iter()
                .any(|note| note.starts_with("game_attacks4:") && note.contains("charge_frame")),
            "{:?}",
            forward.notes
        );
        // What could be resolved still is.
        assert!(
            forward.body.contains("frame(agent.lua_state_agent, 21.0);"),
            "{}",
            forward.body
        );

        let reverse = index.script_source("pacman", "attacks4reverse").unwrap();
        assert!(
            reverse
                .notes
                .iter()
                .any(|note| note.contains("frame(agent.lua_state_agent, hit_frame);")),
            "{:?}",
            reverse.notes
        );
        assert!(reverse.body.contains("hit_frame"), "nothing is guessed: {}", reverse.body);
    }

    #[test]
    fn a_private_pass_through_body_is_written_where_it_lives() {
        let acmd = MARIO.replace(
            "unsafe extern \"C\" fn game_attackairn(agent: &mut L2CAgentBase) {",
            "unsafe extern \"C\" fn game_attackairn(agent: &mut L2CAgentBase) {\n    \
             nair_impl(agent);\n}\n\nunsafe fn nair_impl(agent: &mut L2CAgentBase) {",
        );
        let tmp = tempfile::tempdir().unwrap();
        let file = write(tmp.path(), "src/mario/acmd.rs", &acmd);
        write(
            tmp.path(),
            "src/mario/mod.rs",
            "pub fn install() { let agent = &mut smashline::Agent::new(\"mario\"); }",
        );
        let index = SourceIndex::build(tmp.path()).unwrap();
        let body = index.script_source("mario", "attack_air_n").unwrap().body;
        assert!(body.contains("fn game_attackairn("), "{body}");
        assert!(body.contains("macros::ATTACK("), "{body}");

        let pristine = crate::acmd::parse_acmd_script(&body).to_hitboxes();
        let mut edited = pristine.clone();
        edited[0].damage = 9.5;
        let report = sync_hitboxes(&index, "mario", "attack_air_n", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1);
        let on_disk = std::fs::read_to_string(&file).unwrap();
        assert!(on_disk.contains("    nair_impl(agent);\n"), "the wrapper is untouched");
        assert!(on_disk.contains("Hash40::new(\"top\"), 9.5, 361"), "{on_disk}");
    }

    #[test]
    fn constant_products_fold_only_where_nothing_binds_tighter() {
        assert_eq!(
            fold_constant_products("f(0, 28.4 * -1.0, Some(12.0 * -1.0), -7.0 * -1.0, 2 * 3)"),
            "f(0, -28.4, Some(-12.0), 7.0, 6)"
        );
        assert_eq!(fold_constant_products("x / 2.0 * 3.0"), "x / 2.0 * 3.0");
        assert_eq!(fold_constant_products("x - 2.0 * 3.0"), "x - 6.0");
        assert_eq!(fold_constant_products("a, *CONST, 1.0"), "a, *CONST, 1.0");
    }

    fn mario_project() -> (tempfile::TempDir, SourceIndex) {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "src/mario/acmd.rs", MARIO);
        write(
            tmp.path(),
            "src/mario/mod.rs",
            "pub fn install() { let agent = &mut smashline::Agent::new(\"mario\"); }",
        );
        // Must be ignored: build output, and a file naming no fighter.
        write(tmp.path(), "target/debug/build.rs", MARIO);
        let index = SourceIndex::build(tmp.path()).unwrap();
        (tmp, index)
    }

    #[test]
    fn a_whole_project_indexes_split_files_and_writes_back_to_the_owning_file() {
        let tmp = tempfile::tempdir().unwrap();
        let game_file = write(
            tmp.path(),
            "src/mario/neutral/air.rs",
            r#"unsafe extern "C" fn custom_neutral_game(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
}
"#,
        );
        let effect_file = write(
            tmp.path(),
            "src/mario/neutral/visuals.rs",
            r#"unsafe extern "C" fn custom_neutral_effect(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
}
"#,
        );
        // Registrations live in a third file and use names that do not carry a category prefix.
        // The project-wide registration pass is what joins these functions back to their scripts.
        let install_file = write(
            tmp.path(),
            "src/mario/install.rs",
            r#"pub fn install() {
    let agent = &mut smashline::Agent::new("mario");
    agent.acmd("game_attackairn", custom_neutral_game, smashline::Priority::Default);
    agent.acmd("effect_attackairn", custom_neutral_effect, smashline::Priority::Default);
}
"#,
        );

        let index = SourceIndex::build(tmp.path()).unwrap();
        let game = index.script("mario", "game_attackairn").unwrap();
        let effect = index.script("mario", "effect_attackairn").unwrap();
        assert_eq!(game.file, game_file);
        assert_eq!(game.function, "custom_neutral_game");
        assert_eq!(effect.file, effect_file);
        assert_eq!(effect.function, "custom_neutral_effect");
        assert_eq!(index.conflict_count(), 0);
        let body = index.script_source("mario", "attack_air_n").unwrap().body;
        // Both functions are in the merged body, under the names they were registered as —
        // see `custom_function_names_still_read_as_their_registered_scripts` for why the
        // header is respelled on the way to the parsers.
        assert!(body.contains("fn game_attackairn"));
        assert!(body.contains("fn effect_attackairn"));
        assert!(body.contains("frame(agent.lua_state_agent, 5.0)"));

        let install_before = std::fs::read_to_string(&install_file).unwrap();
        let report = sync_script(&index, "mario", "game_attackairn", |body| {
            Ok((
                body.replace("5.0", "6.0"),
                SyncReport {
                    changed: 1,
                    ..SyncReport::default()
                },
            ))
        })
        .unwrap();
        assert_eq!(report.files, vec![game_file.clone()]);
        assert!(std::fs::read_to_string(&game_file)
            .unwrap()
            .contains("frame(agent.lua_state_agent, 6.0)"));
        assert!(std::fs::read_to_string(&effect_file)
            .unwrap()
            .contains("frame(agent.lua_state_agent, 4.0)"));
        assert_eq!(
            std::fs::read_to_string(&install_file).unwrap(),
            install_before
        );
    }

    #[test]
    fn nested_modules_inherit_their_fighters_parent_source_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let script_file = write(
            tmp.path(),
            "src/fighters/mario/acmd/aerials/neutral.rs",
            r#"unsafe extern "C" fn custom_neutral(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
}
"#,
        );
        write(
            tmp.path(),
            "src/fighters/mario/mod.rs",
            r#"pub fn install() {
    let agent = &mut smashline::Agent::new("mario");
    agent.acmd("game_attackairn", custom_neutral, smashline::Priority::Default);
}
"#,
        );
        // A second fighter makes project-wide fallback unavailable. The nested module scope is
        // therefore the only safe attribution route for the first function.
        write(
            tmp.path(),
            "src/fighters/luigi/mod.rs",
            r#"pub fn install() { let agent = &mut smashline::Agent::new("luigi"); }
"#,
        );

        let index = SourceIndex::build(tmp.path()).unwrap();
        let site = index.script("mario", "game_attackairn").unwrap();
        assert_eq!(site.file, script_file);
        assert_eq!(site.function, "custom_neutral");
        assert!(index.script("luigi", "game_attackairn").is_none());

        let report = sync_script(&index, "mario", "game_attackairn", |body| {
            Ok((
                body.replace("5.0", "6.0"),
                SyncReport {
                    changed: 1,
                    ..SyncReport::default()
                },
            ))
        })
        .unwrap();
        assert_eq!(report.files, vec![script_file.clone()]);
        assert!(std::fs::read_to_string(script_file)
            .unwrap()
            .contains("6.0"));
    }

    /// Empoleon's layout: the module is named for the mod (`emperte/`), not the fighter it
    /// plays on; the ACMD folder also declares Falco's articles; the mod's folder hooks the
    /// `common` agent. None of that may take Falco's own scripts away from him.
    #[test]
    fn a_module_named_for_the_mod_still_belongs_to_its_base_fighter() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "src/lib.rs",
            r#"mod emperte;
pub fn main() {
    let agent = &mut Agent::new("falco");
    emperte::install(agent);
    agent.install();
}
"#,
        );
        write(
            tmp.path(),
            "src/emperte/mod.rs",
            "mod acmd;\nmod vars;\npub fn install(agent: &mut Agent) { acmd::install(agent); }\n",
        );
        write(
            tmp.path(),
            "src/emperte/vars.rs",
            "pub fn install() { Agent::new(\"common\").on_line(Main, hook).install(); }\n",
        );
        write(
            tmp.path(),
            "src/emperte/acmd/mod.rs",
            "mod aerials;\nmod specials;\npub fn install(agent: &mut Agent) { aerials::install(agent); specials::install(agent); }\n",
        );
        let aerials = write(
            tmp.path(),
            "src/emperte/acmd/aerials.rs",
            r#"unsafe extern "C" fn game_attackairb(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
}
pub fn install(agent: &mut Agent) {
    agent.acmd("game_attackairb", game_attackairb, Priority::Low);
}
"#,
        );
        write(
            tmp.path(),
            "src/emperte/acmd/specials.rs",
            r#"unsafe extern "C" fn game_specialnstart(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 3.0);
}
unsafe extern "C" fn game_fly(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 1.0);
}
pub fn install(agent: &mut Agent) {
    agent.acmd("game_specialnstart", game_specialnstart, Priority::Low);
    Agent::new("falco_cannon").acmd("game_fly", game_fly, Priority::Low).install();
    Agent::new("falco_arwing").install();
}
"#,
        );

        write(
            tmp.path(),
            "Cargo.toml",
            "[package]
name = \"empoleon\"

[lib]
crate-type = [\"cdylib\"]
",
        );
        // A backup crate kept beside the live one; cargo never builds it.
        write(tmp.path(), "plugin(old)/Cargo.toml", "[package]
name = \"empoleon\"
");
        write(tmp.path(), "plugin(old)/src/lib.rs", "mod aerials;
");
        write(
            tmp.path(),
            "plugin(old)/src/aerials.rs",
            "unsafe extern \"C\" fn game_attackairb(agent: &mut L2CAgentBase) {}
",
        );

        // Linked at the moveset, its crate's `src/`, or the mod's own folder: the folder
        // linked inside the crate is told it is Falco's only by `src/lib.rs` above it.
        for linked in ["", "src", "src/emperte"] {
            let index = SourceIndex::build(&tmp.path().join(linked)).unwrap();
            assert_eq!(
                index.script("falco", "game_attackairb").unwrap().file,
                aerials,
                "linked at {linked:?}"
            );
            assert!(index.script("falco", "game_specialnstart").is_some(), "linked at {linked:?}");
            assert!(index.script("falco_cannon", "game_fly").is_some(), "linked at {linked:?}");
            assert!(index.script("falco_cannon", "game_attackairb").is_none(), "linked at {linked:?}");
            assert!(!index.has_fighter("common"), "linked at {linked:?}");
            assert!(index.conflicts.is_empty(), "linked at {linked:?}");
        }
    }

    /// Empoleon installs Falco's agent narrowed to its own costumes, through a rebinding:
    /// `let agent = agent.set_costume(…)`. Its scripts are Empoleon's slots', not Falco's own,
    /// wherever the folder is linked — the gating is read from the crate root above it.
    #[test]
    fn a_costume_narrowed_agent_marks_its_fighter_costume_gated() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "Cargo.toml",
            "[package]
name = \"empoleon\"

[lib]
crate-type = [\"cdylib\"]
",
        );
        write(
            tmp.path(),
            "src/lib.rs",
            r#"mod emperte;
pub fn main() {
    // let other = Agent::new("mario").set_costume(all.to_vec());
    let agent_emperte = &mut Agent::new("falco");
    let agent_emperte = agent_emperte.set_costume(costume.to_vec());
    emperte::install(agent_emperte);
    agent_emperte.install();
    Agent::new("falco_cannon").install();
}
"#,
        );
        write(
            tmp.path(),
            "src/emperte/mod.rs",
            r#"unsafe extern "C" fn game_attackairb(agent: &mut L2CAgentBase) {}
pub fn install(agent: &mut Agent) {
    agent.acmd("game_attackairb", game_attackairb, Priority::Low);
}
"#,
        );
        for linked in ["", "src", "src/emperte"] {
            let index = SourceIndex::build(&tmp.path().join(linked)).unwrap();
            assert!(index.is_costume_gated("falco"), "linked at {linked:?}");
            assert!(index.is_costume_gated("fighter_falco"), "attribute spelling");
            assert!(!index.is_costume_gated("falco_cannon"), "linked at {linked:?}");
            assert!(!index.is_costume_gated("mario"), "a commented-out line is not code");
            assert!(index.script("falco", "game_attackairb").is_some(), "linked at {linked:?}");
        }
    }

    #[test]
    fn costume_gating_reads_chains_and_stays_inside_the_binding() {
        let gating = scan_agent_gating(
            r#"pub fn a() {
    Agent::new("falco")
        .set_costume(costume.to_vec())
        .acmd("game_attack11", attack11, Priority::Low)
        .install();
}
pub fn b() {
    let agent = &mut Agent::new("mario");
    agent.install();
}
pub fn c() {
    let agent = &mut Agent::new("luigi");
    agent.set_costume(slots);
    agent.install();
}
pub fn d() -> Agent { Agent::new("pikachu").set_costume(slots) }
pub fn e() { let x = Agent::new("common"); x.set_costume(slots); }
"#,
        );
        assert_eq!(
            gating,
            vec![
                ("falco".to_string(), true),
                ("mario".to_string(), false),
                ("luigi".to_string(), true),
                ("pikachu".to_string(), true),
            ]
        );
    }

    /// One agent for every costume beside a narrowed one modifies the fighter's own slots too.
    #[test]
    fn an_ungated_agent_beside_a_gated_one_is_not_costume_gated() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "acmd.rs",
            r#"unsafe extern "C" fn game_attack11(agent: &mut L2CAgentBase) {}
pub fn install() {
    Agent::new("falco").set_costume(slots).acmd("game_attack11", game_attack11, Priority::Low).install();
    Agent::new("falco").on_start(start).install();
}
"#,
        );
        let index = SourceIndex::build(tmp.path()).unwrap();
        assert!(!index.is_costume_gated("falco"));
    }

    /// A project whose functions are named for the mod, not the script, has to read the same
    /// as one that follows the convention. The registration is what says which script the
    /// function is, and the merged body has to carry that name — the parsers split a body by
    /// `fn game_`/`fn effect_`/…, so a custom name reads as no move at all the moment any other
    /// category shares the body.
    #[test]
    fn custom_function_names_still_read_as_their_registered_scripts() {
        let tmp = tempfile::tempdir().unwrap();
        let game_file = write(
            tmp.path(),
            "src/mario/nair.rs",
            r#"unsafe extern "C" fn hudsons_nair(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("top"), 8.0, 361, 100, 0, 40, 4.5, 0.0, 8.0, 6.0, None, None, None, 1.0, 1.0, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_POS, false, 0, 0.0, 0, false, false, false, false, false, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_M, *COLLISION_SOUND_ATTR_PUNCH, *ATTACK_REGION_PUNCH);
    }
}

unsafe extern "C" fn hudsons_nair_vfx(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_FLIP(agent, Hash40::new("sys_hit_l"), Hash40::new("sys_hit_r"), Hash40::new("haver"), 1.0, 2.0, 3.0, 0.0, 90.0, 45.0, 1.5, true, *EF_FLIP_YZ);
    }
}

pub fn install(agent: &mut smashline::Agent) {
    agent.acmd("game_attackairn", hudsons_nair, smashline::Priority::Default);
    agent.acmd("effect_attackairn", hudsons_nair_vfx, smashline::Priority::Default);
}
"#,
        );

        let index = SourceIndex::build(tmp.path()).unwrap();
        let project = index.script_source("mario", "attack_air_n").unwrap();
        assert_eq!(project.covers, ["game_", "effect_"]);
        // The mirror's `sound_` joins the body, which is what turns the whole-text fallback in
        // `parse_acmd_script` off: before the header was renamed, this is where the hitbox and
        // the effect both disappeared.
        let merged = crate::acmd::merge_project_over_mirror_blocked(
            &project.body,
            &project.covers,
            &project.blocked,
            r#"unsafe extern "C" fn sound_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 2.0);
}
"#,
        );
        assert_eq!(
            crate::acmd::parse_acmd_script(&merged).to_hitboxes().len(),
            1
        );
        assert_eq!(
            crate::acmd::parse_effect_script(&merged).stmts.len(),
            2,
            "the effect function is read under its registered name too"
        );
        assert!(!crate::acmd::parse_sound_script(&merged).stmts.is_empty());

        // Renaming is for the copy on screen only: the write path patches the file by span, so
        // the user's own names survive a sync.
        sync_script(&index, "mario", "game_attackairn", |body| {
            Ok((
                body.replace("5.0", "6.0"),
                SyncReport {
                    changed: 1,
                    ..SyncReport::default()
                },
            ))
        })
        .unwrap();
        let after = std::fs::read_to_string(&game_file).unwrap();
        assert!(after.contains("fn hudsons_nair(agent"));
        assert!(after.contains("frame(agent.lua_state_agent, 6.0)"));
    }

    #[test]
    fn one_unlabelled_source_set_is_available_without_guessing_a_fighter_name() {
        let tmp = tempfile::tempdir().unwrap();
        let game_file = write(
            tmp.path(),
            "scripts/combat.rs",
            r#"unsafe extern "C" fn game_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
}
"#,
        );
        let effect_file = write(
            tmp.path(),
            "scripts/visuals.rs",
            r#"unsafe extern "C" fn effect_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
}
"#,
        );

        let index = SourceIndex::build(tmp.path()).unwrap();
        assert!(index.has_unattributed_scripts());
        assert!(index.has_fighter("mario"));
        assert_eq!(
            index.script("mario", "game_attackairn").unwrap().file,
            game_file
        );
        assert_eq!(
            index.script("mario", "effect_attackairn").unwrap().file,
            effect_file
        );
        let project = index.script_source("mario", "attack_air_n").unwrap();
        assert_eq!(project.covers, ["game_", "effect_"]);
        assert!(project.body.contains("fn game_attackairn"));
        assert!(project.body.contains("fn effect_attackairn"));
    }

    #[test]
    fn a_single_named_project_remains_the_fallback_for_its_unscoped_functions() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "scripts/unowned.rs",
            "unsafe extern \"C\" fn game_attackairn(agent: &mut L2CAgentBase) { }",
        );
        write(
            tmp.path(),
            "src/mario/mod.rs",
            "pub fn install() { let agent = &mut smashline::Agent::new(\"mario\"); }",
        );

        let index = SourceIndex::build(tmp.path()).unwrap();
        assert!(!index.has_unattributed_scripts());
        assert!(index.script("mario", "game_attackairn").is_some());
    }

    #[test]
    fn duplicate_script_identities_are_reported_and_never_filled_from_the_mirror() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "src/mario/first.rs",
            r#"#[acmd_script(agent = "fighter_mario", script = "game_attackairn", category = ACMD_GAME)]
unsafe extern "C" fn first_game(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 3.0);
}
"#,
        );
        write(
            tmp.path(),
            "src/mario/second.rs",
            r#"#[acmd_script(agent = "fighter_mario", script = "game_attackairn", category = ACMD_GAME)]
unsafe extern "C" fn second_game(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 7.0);
}
"#,
        );

        let index = SourceIndex::build(tmp.path()).unwrap();
        assert!(index.script("mario", "game_attackairn").is_none());
        assert_eq!(index.conflict_count(), 1);
        let conflict = index.conflict("mario", "game_attackairn").unwrap();
        assert_eq!(conflict.sites.len(), 2);
        assert!(conflict
            .sites
            .iter()
            .any(|site| site.function == "first_game"));
        assert!(conflict
            .sites
            .iter()
            .any(|site| site.function == "second_game"));
        assert!(conflict.description().contains("first.rs::first_game"));
        assert!(conflict.description().contains("second.rs::second_game"));

        let project = index.script_source("mario", "attack_air_n").unwrap();
        assert!(project.body.is_empty());
        assert!(project.covers.is_empty());
        assert_eq!(project.blocked, ["game_"]);
        assert!(
            project.needs_mirror(),
            "the other missing categories may still be filled from the mirror"
        );
        let merged = crate::acmd::merge_project_over_mirror_blocked(
            &project.body,
            &project.covers,
            &project.blocked,
            "unsafe extern \"C\" fn game_attackairn(agent: &mut L2CAgentBase) { }",
        );
        assert!(merged.is_empty(), "an ambiguous category must stay blocked");

        let error = create_script(&index, "mario", "game_attackairn", "")
            .expect_err("creation must not add a third copy to an ambiguous identity")
            .to_string();
        assert!(error.contains("ambiguous"), "{error}");
        assert!(
            error.contains("first_game") && error.contains("second_game"),
            "{error}"
        );
    }

    #[test]
    fn a_smashline_project_indexes_its_scripts_by_fighter_and_move() {
        let (_tmp, index) = mario_project();
        assert!(index.has_fighter("mario"));
        assert!(index.script("mario", "game_attackairn").is_some());
        assert!(index.script("mario", "effect_attackairn").is_some());
        assert_eq!(index.script_count(), 2, "target/ must not be indexed");

        let body = index.script_source("mario", "attack_air_n").unwrap().body;
        assert!(body.contains("fn game_attackairn") && body.contains("fn effect_attackairn"));
        // The whole point: the parsers see the user's macro, not vanilla's.
        let calls = crate::acmd::parse_effect_script(&body).to_effect_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].spawn_func, "EFFECT_FOLLOW_FLIP");
        assert_eq!(calls[0].effect_name_alt.as_deref(), Some("sys_hit_r"));
    }

    /// A project says which categories it speaks for, and only asks for the mirror when one
    /// the editor displays is missing.
    ///
    /// The sound-only case is the one D1a wrote down and could not fix: `script_body` returned
    /// `None` for it, so the project's own sounds were the single thing that did not survive
    /// loading them.
    #[test]
    fn a_project_reports_the_categories_it_covers_and_asks_for_the_rest() {
        let (_tmp, index) = mario_project();
        let both = index.script_source("mario", "attack_air_n").unwrap();
        assert_eq!(both.covers, ["game_", "effect_"]);
        // **Changed at D1d, and it is the change rather than a regression.** This project has
        // hitboxes and effects but writes no `sound_`, and until sounds were editable that was
        // "everything shown" and no reason to touch the network. Now the sound section would sit
        // empty for it, so the mirror is worth asking for.
        assert!(
            both.needs_mirror(),
            "a project with no sound script of its own needs the mirror's to show one"
        );

        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "src/kirby/acmd.rs",
            "unsafe extern \"C\" fn sound_attackairn(agent: &mut L2CAgentBase) {\n    \
             frame(agent.lua_state_agent, 3.0);\n}\n",
        );
        write(
            tmp.path(),
            "src/kirby/mod.rs",
            "pub fn install() { let agent = &mut smashline::Agent::new(\"kirby\"); }",
        );
        let index = SourceIndex::build(tmp.path()).unwrap();
        let sound_only = index.script_source("kirby", "attack_air_n").unwrap();
        assert_eq!(sound_only.covers, ["sound_"]);
        assert!(sound_only.needs_mirror());

        assert!(
            index.script_source("kirby", "attack_air_f").is_none(),
            "a move the project says nothing about is the mirror's alone"
        );
    }

    /// The merge makes a mirror-sourced effect editable in a project that has no `effect_` of
    /// its own, so write-back has to say so rather than guess where to put it.
    ///
    /// **Still true after D1e, and deliberately so.** Creating the missing function is a
    /// separate step ([`create_script`]) that the caller runs *first*; the value sync's own job
    /// never became "write wherever looks plausible". If this test ever starts passing by
    /// writing something, a sync has grown the power to invent a destination.
    #[test]
    fn syncing_a_category_the_project_does_not_define_refuses_instead_of_guessing() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "src/kirby/acmd.rs",
            "unsafe extern \"C\" fn game_attackairn(agent: &mut L2CAgentBase) {\n    \
             frame(agent.lua_state_agent, 3.0);\n}\n",
        );
        write(
            tmp.path(),
            "src/kirby/mod.rs",
            "pub fn install() { let agent = &mut smashline::Agent::new(\"kirby\"); }",
        );
        let index = SourceIndex::build(tmp.path()).unwrap();

        let error = sync_effect_calls(&index, "kirby", "attack_air_n", &[], &[])
            .expect_err("a project with no effect_ must not be written into")
            .to_string();
        assert!(
            error.contains("effect_attackairn"),
            "the message has to name what is missing: {error}"
        );
    }

    /// A sound the project does not define is created beside the scripts it does, registered the
    /// same way, and comes back under exactly the name it was asked for.
    ///
    /// **The re-index is the assertion that matters**, not the file's text. A function installed
    /// under the wrong script name is still valid Rust that compiles and runs — it simply
    /// replaces a move nobody edited. Only asking the indexer what it now sees catches that.
    #[test]
    fn a_missing_sound_script_is_created_beside_its_siblings_and_registered_with_them() {
        let (tmp, index) = mario_project();
        let source = SOUNDS.replace("sound_turndash", "sound_attackairn");

        let created = create_script(&index, "mario", "sound_attackairn", &source).unwrap();
        assert!(
            created.note.contains("registered it beside"),
            "{}",
            created.note
        );

        let after = SourceIndex::build(tmp.path()).unwrap();
        let site = after
            .script("mario", "sound_attackairn")
            .expect("the created script has to index under the name it was created as");
        let text = std::fs::read_to_string(&created.file).unwrap();
        assert_eq!(
            &text[site.span.clone()],
            source.trim_end(),
            "the function is the mirror's text verbatim — creation writes no edit of its own"
        );
        assert!(
            text.contains(
                "agent.acmd(\"sound_attackairn\", sound_attackairn, smashline::Priority::Default);"
            ),
            "the registration copies the sibling's tail as well as its shape:\n{text}"
        );
        // The siblings still resolve: an insertion moves spans, and a rescan is what makes that
        // safe. This is the assertion that fails if creation ever patches spans by hand instead.
        assert!(after.script("mario", "game_attackairn").is_some());
        assert!(after.script("mario", "effect_attackairn").is_some());
        assert_eq!(after.script_count(), 3);
    }

    /// With no sibling for this move, any script of the same fighter is anchor enough.
    #[test]
    fn a_move_the_project_says_nothing_about_still_has_somewhere_to_be_created() {
        let (tmp, index) = mario_project();
        create_script(&index, "mario", "sound_turndash", SOUNDS).unwrap();

        let after = SourceIndex::build(tmp.path()).unwrap();
        assert!(after.script("mario", "sound_turndash").is_some());
        assert!(
            after.script_source("mario", "turn_dash").is_some(),
            "and the move now reads back out of the project"
        );
    }

    /// An attribute-registered project gets an attribute, with the category derived from how it
    /// spells its own.
    #[test]
    fn creating_into_an_attribute_project_respells_the_attribute_for_the_new_category() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "src/lib.rs",
            "#[acmd_script(agent = \"fighter_lucina\", script = \"game_turndash\", \
             category = ACMD_GAME)]\nunsafe extern \"C\" fn my_custom_name(agent: &mut \
             L2CAgentBase) {\n    frame(agent.lua_state_agent, 1.0);\n}\n",
        );
        let index = SourceIndex::build(tmp.path()).unwrap();

        let created = create_script(&index, "lucina", "sound_turndash", SOUNDS).unwrap();
        let text = std::fs::read_to_string(&created.file).unwrap();
        assert!(
            text.contains(
                "#[acmd_script(agent = \"fighter_lucina\", script = \"sound_turndash\", \
                 category = ACMD_SOUND)]"
            ),
            "the agent is carried, the script renamed, the category derived:\n{text}"
        );
        let after = SourceIndex::build(tmp.path()).unwrap();
        assert!(after.script("lucina", "sound_turndash").is_some());
        assert!(after.script("lucina", "game_turndash").is_some());
    }

    /// The category is only derivable while the project agrees with itself about its own.
    ///
    /// There is no copy of the `#[acmd_script]` macro on this machine, so `ACMD_SOUND` is a
    /// name read off the project rather than one looked up. A project that spells its `game_`
    /// script something else has told us the rule does not hold there, and inventing the token
    /// anyway would install the function under the wrong category — which compiles.
    #[test]
    fn an_unfamiliar_category_token_is_refused_rather_than_guessed_at() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "src/lib.rs",
            "#[acmd_script(agent = \"fighter_lucina\", script = \"game_turndash\", \
             category = Acmd::Game)]\nunsafe extern \"C\" fn my_custom_name(agent: &mut \
             L2CAgentBase) {\n    frame(agent.lua_state_agent, 1.0);\n}\n",
        );
        let index = SourceIndex::build(tmp.path()).unwrap();
        let before = std::fs::read_to_string(tmp.path().join("src/lib.rs")).unwrap();

        let error = create_script(&index, "lucina", "sound_turndash", SOUNDS)
            .expect_err("an unrecognised category token must not be extrapolated")
            .to_string();
        assert!(error.contains("Acmd::Game"), "say what it saw: {error}");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("src/lib.rs")).unwrap(),
            before,
            "and a refusal writes nothing at all"
        );
    }

    /// A project that registers nothing Visionary can see gets a function named the same
    /// conventional way — and is told so, because that is the case where creating one could
    /// silently do nothing.
    #[test]
    fn creating_beside_a_conventionally_named_script_says_it_could_not_register_it() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "src/kirby/acmd.rs",
            "unsafe extern \"C\" fn game_turndash(agent: &mut L2CAgentBase) {\n    \
             frame(agent.lua_state_agent, 3.0);\n}\n",
        );
        write(
            tmp.path(),
            "src/kirby/mod.rs",
            "pub fn install() { let agent = &mut smashline::Agent::new(\"kirby\"); }",
        );
        let index = SourceIndex::build(tmp.path()).unwrap();

        let created = create_script(&index, "kirby", "sound_turndash", SOUNDS).unwrap();
        assert!(
            created.note.contains("check it installs"),
            "a registration that could not be copied has to be said out loud: {}",
            created.note
        );
        let after = SourceIndex::build(tmp.path()).unwrap();
        assert!(after.script("kirby", "sound_turndash").is_some());
    }

    /// Creating a script that already exists is refused, because the failure is not a wasted
    /// write — it is two functions with one name, and a project that stops compiling.
    #[test]
    fn creating_a_script_the_project_already_has_is_refused() {
        let (tmp, index) = mario_project();
        let before = std::fs::read_to_string(tmp.path().join("src/mario/acmd.rs")).unwrap();
        let error = create_script(&index, "mario", "game_attackairn", SOUNDS)
            .expect_err("a second copy of a function is a duplicate definition, not an edit")
            .to_string();
        assert!(error.contains("game_attackairn"), "{error}");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("src/mario/acmd.rs")).unwrap(),
            before
        );
    }

    /// The new function goes beside the same *move*, not beside whatever sorts first.
    ///
    /// Both files here would produce a script that works, so nothing downstream can tell them
    /// apart — which is exactly why the choice needs its own assertion. A project keeps its
    /// moves in separate files for its own reasons, and dropping the aerial's sound into the
    /// specials file is a change the user did not ask for and has to go and undo.
    #[test]
    fn a_created_script_lands_in_the_file_that_holds_the_rest_of_its_move() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "src/mario/aerials.rs",
            "unsafe extern \"C\" fn game_attackairn(agent: &mut L2CAgentBase) {\n    \
             frame(agent.lua_state_agent, 3.0);\n}\n",
        );
        // Sorts before `game_attackairn`, so an anchor chosen by name alone picks this one.
        write(
            tmp.path(),
            "src/mario/specials.rs",
            "unsafe extern \"C\" fn effect_specialn(agent: &mut L2CAgentBase) {\n    \
             frame(agent.lua_state_agent, 3.0);\n}\n",
        );
        write(
            tmp.path(),
            "src/mario/mod.rs",
            "pub fn install() { let agent = &mut smashline::Agent::new(\"mario\"); }",
        );
        let index = SourceIndex::build(tmp.path()).unwrap();

        let created = create_script(&index, "mario", "sound_attackairn", SOUNDS).unwrap();
        assert_eq!(created.file.file_name().unwrap(), "aerials.rs");
    }

    /// An attribute that is not `#[acmd_script]` is not a registration.
    ///
    /// Mistaking one for a registration is the quiet failure this whole path exists to avoid:
    /// the function is written with an `#[allow]` copied above it, the real registration is
    /// never looked for, and the result compiles, installs nothing, and plays vanilla.
    #[test]
    fn an_unrelated_attribute_above_a_sibling_is_not_read_as_its_registration() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "src/mario/acmd.rs",
            "#[allow(unused_variables)]\nunsafe extern \"C\" fn game_turndash(agent: &mut \
             L2CAgentBase) {\n    frame(agent.lua_state_agent, 3.0);\n}\n\npub fn \
             install(agent: &mut smashline::Agent) {\n    agent.acmd(\"game_turndash\", \
             game_turndash, smashline::Priority::Default);\n}\n",
        );
        write(
            tmp.path(),
            "src/mario/mod.rs",
            "pub fn install() { let agent = &mut smashline::Agent::new(\"mario\"); }",
        );
        let index = SourceIndex::build(tmp.path()).unwrap();

        let created = create_script(&index, "mario", "sound_turndash", SOUNDS).unwrap();
        let text = std::fs::read_to_string(&created.file).unwrap();
        assert!(
            !text.contains("#[allow(unused_variables)]\nunsafe extern \"C\" fn sound_turndash"),
            "an #[allow] is not a registration:\n{text}"
        );
        assert!(
            text.contains("agent.acmd(\"sound_turndash\", sound_turndash,"),
            "the real registration is the one below it:\n{text}"
        );
    }

    /// No script for this fighter at all is still a refusal: there is no file to write into, no
    /// fighter attribution to inherit, and no registration style to copy.
    #[test]
    fn creating_for_a_fighter_the_project_never_mentions_is_refused() {
        let (_tmp, index) = mario_project();
        let error = create_script(&index, "kirby", "sound_turndash", SOUNDS)
            .expect_err("a fighter the project says nothing about is out of scope")
            .to_string();
        assert!(error.contains("sound_turndash"), "{error}");
    }

    /// Creation writes vanilla, the ordinary sync writes the edit, and the two together leave a
    /// function that differs from the mirror's on exactly one line.
    ///
    /// This is the whole point of splitting them: creation never learns what an edit is, and the
    /// value write never learns the function is new. A regenerating creator would have rewritten
    /// every line here and passed a round-trip while doing it.
    #[test]
    fn a_created_script_then_synced_differs_from_the_mirror_only_where_it_was_edited() {
        let (tmp, index) = mario_project();
        let source = SOUNDS.replace("sound_turndash", "sound_attackairn");
        create_script(&index, "mario", "sound_attackairn", &source).unwrap();
        let index = SourceIndex::build(tmp.path()).unwrap();

        let pristine = sounds_of(&source);
        let mut edited = pristine.clone();
        edited[0].call.sounds[0] = "se_mario_dash_start".into();
        let report = sync_sounds(&index, "mario", "attack_air_n", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1);
        assert!(report.skipped.is_empty(), "{:?}", report.skipped);

        let site = index.script("mario", "sound_attackairn").unwrap();
        let written = std::fs::read_to_string(&site.file).unwrap();
        let written = &written[site.span.clone()];
        let differing: Vec<_> = written
            .lines()
            .zip(source.lines())
            .filter(|(now, was)| now != was)
            .collect();
        assert_eq!(differing.len(), 1, "{differing:?}");
        assert!(differing[0].0.contains("se_mario_dash_start"));
    }

    #[test]
    fn a_fighter_named_only_by_an_attribute_still_resolves() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "src/lib.rs",
            r#"
#[acmd_script(agent = "fighter_lucina", script = "game_attacks4", category = ACMD_GAME)]
unsafe extern "C" fn my_custom_name(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 1.0);
}
"#,
        );
        let index = SourceIndex::build(tmp.path()).unwrap();
        // `fighter_lucina` and `lucina` are the same character.
        assert!(index.script("lucina", "game_attacks4").is_some());
    }

    #[test]
    fn scanning_ignores_macro_names_inside_comments_and_strings() {
        let text = r#"fn f() {
    // macros::EFFECT(agent, 1);
    let s = "macros::ATTACK(agent, 2);";
    /* macros::EFFECT_FOLLOW(agent, 3); */
    macros::EFFECT(agent, 4, 5);
}"#;
        let sites = scan_macro_sites(text, 0..text.len());
        assert_eq!(sites.len(), 1, "found {sites:?}");
        assert_eq!(sites[0].name, "EFFECT");
        assert_eq!(sites[0].arg(text, 1), Some("4"));
    }

    /// Shy Guy's forward smash, shape for shape: a retired `EFFECT_FOLLOW` and a whole trail
    /// block left as `//` lines between live calls. The parser used to read both as calls, so
    /// the editor drew seven effects and every edit after the comments matched the wrong site.
    const COMMENTED_EFFECT_SOURCE: &str = r#"unsafe extern "C" fn effect_attacks4(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_smash_flash"), Hash40::new("top"), 12, 10, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, false);
    }
    frame(agent.lua_state_agent, 17.0);
    if macros::is_excute(agent) {
        // Dedede's hammer arc.
        macros::EFFECT(agent, Hash40::new("shyguy_mace_arc_wind"), Hash40::new("top"), 2, 4, 10, 0, -60, -115, 0.85, 0, 0, 0, 0, 0, 0, true);
        // macros::EFFECT_FOLLOW(agent, Hash40::new("shyguy_mace_arc_wind"), Hash40::new("haver"), 2, 4, 10, 0, -60, -115, 0.85, true);
    }
    // frame(agent.lua_state_agent, 15.0);
    // if macros::is_excute(agent) {
    //     let (ball, z) = if is_facing_left(agent.module_accessor) {
    //         (Hash40::new("center_boner"), 3.0)
    //     } else {
    //         (Hash40::new("center_bonel"), -3.0)
    //     };
    //     macros::AFTER_IMAGE4_ON_arg29(agent, Hash40::new("tex_shyguy_hammer"), Hash40::new("tex_shyguy_cutlass2"), 10, ball, 0, 3.0, -z, ball, 0, -3.0, z, true, Hash40::new("null"), ball, 0, 0, 0, 0, 0, 0, 1, 0, *EFFECT_AXIS_X, 0, *TRAIL_BLEND_ALPHA, 101, *TRAIL_CULL_NONE, 1.4, 0.2);
    // }
    frame(agent.lua_state_agent, 20.0);
    if macros::is_excute(agent) {
        macros::LANDING_EFFECT(agent, Hash40::new("sys_h_smoke_b"), Hash40::new("top"), 0, 0, -7.0, 0, 0, 0, 0.7, 0, 0, 0, 0, 0, 0, true);
        // // Stop adding to it before the spin flips it
        // macros::AFTER_IMAGE_OFF(agent, 10);
        /* macros::EFFECT(agent, Hash40::new("sys_block_comment"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, true); */
        /*
        macros::EFFECT(agent, Hash40::new("sys_spanning_comment"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, true);
        if x {
        */
    }
    frame(agent.lua_state_agent, 21.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_attack_impact"), Hash40::new("top"), 0, 11.6, 28.4, 90, 0, 0, 1.95, 0, 0, 0, 0, 0, 0, true);
    }
    frame(agent.lua_state_agent, 24.0);
    if macros::is_excute(agent) {
        macros::EFFECT_DETACH_KIND(agent, Hash40::new("shyguy_mace_arc_wind"), -1);
    }
}
"#;

    #[test]
    fn commented_out_effect_calls_are_not_read_as_calls() {
        let calls = crate::acmd::parse_effect_script(COMMENTED_EFFECT_SOURCE).to_effect_calls();
        let names: Vec<&str> = calls.iter().map(|call| call.effect_name.as_str()).collect();
        assert_eq!(calls.len(), 5, "{names:?}");
        assert!(!names.contains(&"tex_shyguy_hammer"), "{names:?}");
        assert!(!names.contains(&"sys_block_comment"), "{names:?}");
        assert!(!names.contains(&"sys_spanning_comment"), "{names:?}");
        let arcs: Vec<_> = calls
            .iter()
            .filter(|call| call.effect_name == "shyguy_mace_arc_wind" && call.active_start == 17)
            .collect();
        assert_eq!(arcs.len(), 1, "{calls:#?}");
        assert!(!arcs[0].follows_bone, "the live arc is the plain EFFECT, not the comment");
        // The structure still closes: the impact lands at its own frame.
        let impact = calls
            .iter()
            .find(|call| call.effect_name == "sys_attack_impact")
            .expect("impact parsed");
        assert_eq!(impact.active_start, 21);
    }

    #[test]
    fn an_edit_after_commented_calls_lands_on_its_own_line() {
        let source = COMMENTED_EFFECT_SOURCE;
        let pristine = crate::acmd::parse_effect_script(source).to_effect_calls();
        let mut edited = pristine.clone();
        let impact = edited
            .iter()
            .position(|call| call.effect_name == "sys_attack_impact")
            .expect("impact parsed");
        edited[impact].offset[1] = 12.5;

        let (after, report) =
            rewrite_effect_calls(source, "pacman/attack_s4_s", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(
            after.contains(r#"Hash40::new("sys_attack_impact"), Hash40::new("top"), 0, 12.5, 28.4,"#),
            "{after}"
        );
        // Everything else, comments included, is untouched.
        let changed: Vec<_> = source
            .lines()
            .zip(after.lines())
            .filter(|(before, after)| before != after)
            .collect();
        assert_eq!(changed.len(), 1, "{changed:#?}");
        let reparsed = crate::acmd::parse_effect_script(&after).to_effect_calls();
        assert_eq!(
            effect_call_set_signature(&reparsed),
            effect_call_set_signature(&edited)
        );
    }

    #[test]
    fn generated_helper_scanner_only_accepts_direct_calls() {
        let text = r#"unsafe fn visionary_set_speed(agent: &mut L2CAgentBase, x: f32, y: f32) {}
// visionary_set_speed(agent, 1, 2);
let string = "visionary_set_speed(agent, 3, 4)";
foo.visionary_set_speed(agent, 5, 6);
macros::visionary_set_speed(agent, 7, 8);
visionary_set_speed(agent, 9, 10);
"#;
        let sites = scan_named_sites(text, "visionary_set_speed", 0..text.len());
        assert_eq!(sites.len(), 1, "found {sites:?}");
        assert_eq!(sites[0].arg(text, 1), Some("9"));
        assert_eq!(sites[0].arg(text, 2), Some("10"));
    }

    /// The scanners walk bytes, so anything multi-byte in the user's file used to be a
    /// chance to slice mid-character and panic the whole editor.
    #[test]
    fn non_ascii_source_does_not_panic_the_scanner() {
        let text = "fn f() {\n    // café — naïve ☃\n    let σ = \"日本語\";\n    \
                    macros::EFFECT(agent, 1, 2);\n}";
        let sites = scan_macro_sites(text, 0..text.len());
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].arg(text, 1), Some("1"));
        // And the same for the function scanner, which brace-matches over the same text.
        assert_eq!(scan_acmd_functions(text).len(), 0);
    }

    #[test]
    fn nested_calls_do_not_split_an_argument() {
        let text = r#"macros::EFFECT(agent, Hash40::new("a"), foo(1, 2), 3.0);"#;
        let sites = scan_macro_sites(text, 0..text.len());
        assert_eq!(sites[0].args.len(), 4);
        assert_eq!(sites[0].arg(text, 1), Some(r#"Hash40::new("a")"#));
        assert_eq!(sites[0].arg(text, 2), Some("foo(1, 2)"));
    }

    #[test]
    fn syncing_a_spawn_rewrites_only_the_values_it_changed() {
        let (tmp, index) = mario_project();
        let body = index.script_source("mario", "attack_air_n").unwrap().body;
        let pristine = crate::acmd::parse_effect_script(&body).to_effect_calls();

        let mut edited = pristine.clone();
        edited[0].offset[1] = 9.25;
        edited[0].scale = 2.0;

        let report =
            sync_effect_calls(&index, "mario", "attack_air_n", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 2, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");

        let after = std::fs::read_to_string(tmp.path().join("src/mario/acmd.rs")).unwrap();
        assert!(
            after.contains(
                r#"macros::EFFECT_FOLLOW_FLIP(agent, Hash40::new("sys_hit_l"), Hash40::new("sys_hit_r"), Hash40::new("haver"), 1.0, 9.25, 3.0, 0.0, 90.0, 45.0, 2.0, true, *EF_FLIP_YZ);"#
            ),
            "the macro, its graphics, and its tail must be untouched:\n{after}"
        );
        // Untouched values keep their original spelling rather than being reformatted.
        assert!(after.contains("1.0, 9.25, 3.0"), "{after}");
        assert!(
            after.contains("// A comment mentioning"),
            "comments survive"
        );
    }

    #[test]
    fn syncing_a_flip_axis_rewrites_only_the_measured_tail_token() {
        let source = r#"unsafe extern "C" fn effect_flip(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_FLIP(agent, Hash40::new("left"), Hash40::new("right"), Hash40::new("top"), 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 1.5, true, *EF_FLIP_YZ);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(source).to_effect_calls();
        let mut edited = pristine.clone();
        edited[0].flip_axis = Some("*EF_FLIP_XY".into());

        let (after, report) =
            rewrite_effect_calls(source, "mario/effect_flip", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        let reparsed = crate::acmd::parse_effect_script(&after).to_effect_calls();
        assert_eq!(
            effect_call_set_signature(&reparsed),
            effect_call_set_signature(&edited),
            "rewritten source:\n{after}\nreparsed: {reparsed:#?}\nedited: {edited:#?}"
        );
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains("1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 1.5, true, *EF_FLIP_XY);"));
        assert!(!after.contains("*EF_FLIP_YZ"));
    }

    /// Retuning the spawn beside a Work ID line must leave that authored token untouched. The
    /// Work ID is a separate source value, not part of the spawn transform.
    #[test]
    fn syncing_a_spawn_does_not_delete_the_work_int_line_beside_it() {
        let source = r#"unsafe extern "C" fn effect_tornadostart(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("metaknight_tornado"), Hash40::new("trans"), 0, 0, 0, 0, 0, 0, 1, false);
        macros::LAST_EFFECT_SET_WORK_INT(agent, *FIGHTER_METAKNIGHT_STATUS_SPECIAL_N_SPIN_WORK_INT_EFFECT_HANDLE);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(source).to_effect_calls();
        let mut edited = pristine.clone();
        edited[0].offset[0] = 2.5;

        let (after, report) =
            rewrite_effect_calls(source, "kirby/tornadostart", &pristine, &edited).unwrap();
        assert_eq!(
            report.changed, 1,
            "only the spawn transform changed: {report:?}"
        );
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(
            after.contains("macros::LAST_EFFECT_SET_WORK_INT(agent, *FIGHTER_METAKNIGHT_STATUS_SPECIAL_N_SPIN_WORK_INT_EFFECT_HANDLE);"),
            "source write-back must not delete the typed C7 line:\n{after}"
        );
        assert_eq!(
            after
                .matches("macros::LAST_EFFECT_SET_WORK_INT(agent,")
                .count(),
            1,
            "the unmodeled line must remain exactly once:\n{after}"
        );
        assert!(
            after.contains(", 2.5, 0, 0, 0, 0, 0, 1, false);"),
            "{after}"
        );
    }

    #[test]
    fn work_int_write_back_changes_only_the_existing_work_id_token() {
        let source = r#"unsafe extern "C" fn effect_work(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_smoke"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, true);
        macros::LAST_EFFECT_SET_WORK_INT(agent, *WORK_INT);
        macros::LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT(agent, 0.25);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(source).to_effect_calls();
        assert_eq!(pristine[0].work_int.as_deref(), Some("WORK_INT"));
        let mut edited = pristine.clone();
        edited[0].work_int = Some("WORK_INT_ALT".into());

        let (after, report) =
            rewrite_effect_calls(source, "test/work_int", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains("LAST_EFFECT_SET_WORK_INT(agent, *WORK_INT_ALT);"));
        assert!(after.contains("LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT(agent, 0.25);"));
        assert_eq!(
            crate::acmd::parse_effect_script(&after).to_effect_calls()[0]
                .work_int
                .as_deref(),
            Some("WORK_INT_ALT")
        );
    }

    /// C4's opaque lifetime controls are not source-editable fields yet. Retuning the spawn they
    /// follow must still leave each command in place, so a source sync cannot turn a preserved
    /// action into a deleted one.
    #[test]
    fn syncing_a_spawn_does_not_delete_opaque_c4_lifetime_lines() {
        let source = r#"unsafe extern "C" fn effect_c4(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_smoke"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, true);
        macros::EFFECT_DETACH_KIND(agent, Hash40::new("sys_smoke"), 0);
        macros::EFFECT_DETACH_KIND_WORK(agent, *WORK_INT, 0);
        macros::ENABLE_AREA(agent, 2);
        macros::UNABLE_AREA(agent, 2);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(source).to_effect_calls();
        let mut edited = pristine.clone();
        edited[0].scale = 1.5;

        let (after, report) = rewrite_effect_calls(source, "test/c4", &pristine, &edited).unwrap();
        assert_eq!(
            report.changed, 1,
            "only the spawn scale changed: {report:?}"
        );
        assert!(report.skipped.is_empty(), "{report:?}");
        for line in [
            "macros::EFFECT_DETACH_KIND(agent, Hash40::new(\"sys_smoke\"), 0);",
            "macros::EFFECT_DETACH_KIND_WORK(agent, *WORK_INT, 0);",
            "macros::ENABLE_AREA(agent, 2);",
            "macros::UNABLE_AREA(agent, 2);",
        ] {
            assert_eq!(
                after.matches(line).count(),
                1,
                "source syncing must retain {line}:\n{after}"
            );
        }
        assert!(after.contains(", 1.5, true);"), "{after}");
    }

    #[test]
    fn native_effect_control_ordinals_keep_a_later_spawn_and_control_retime_aligned() {
        let source = r###"unsafe extern "C" fn effect_native_control(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 2.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_first"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, true);
    }
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        EffectModule::set_disable_render_offset_last(agent.module_accessor);
    }
    frame(agent.lua_state_agent, 8.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_second"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, true);
    }
}
"###;
        let pristine = crate::acmd::parse_effect_script(source).to_effect_calls();
        assert_eq!(pristine.len(), 3);
        assert_eq!(
            spawn_site_names(source),
            vec![
                "EFFECT_FOLLOW".to_string(),
                "EffectModule::set_disable_render_offset_last".to_string(),
                "EFFECT_FOLLOW".to_string(),
            ]
        );

        let mut edited = pristine.clone();
        edited[1].active_start = 7;
        edited[2].offset[0] = 4.0;
        let (after, report) =
            rewrite_effect_calls(source, "test/native_effect_control", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(
            report.changed >= 2,
            "expected retime and transform: {report:?}"
        );
        assert!(
            after.contains("EffectModule::set_disable_render_offset_last(agent.module_accessor);")
        );
        assert!(
            after.contains("Hash40::new(\"sys_second\"), Hash40::new(\"top\"), 4.0, 0, 0"),
            "{after}"
        );
        let reparsed = crate::acmd::parse_effect_script(&after).to_effect_calls();
        assert_eq!(reparsed.len(), 3);
        assert_eq!(reparsed[1].active_start, 7);
        assert_eq!(reparsed[2].offset[0], 4.0);
    }

    #[test]
    fn native_effect_control_scanner_matches_only_direct_typed_statements_for_ordinals() {
        let source = r###"unsafe extern "C" fn effect_native_control_shapes(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        EffectModule::set_disable_render_offset_last(agent.module_accessor);
        EffectModule::enable_sync_init_pos_last();
        EffectModule::set_disable_render_offset_last(boma);
        EffectModule::enable_sync_init_pos_last(agent.module_accessor, 0);
        EffectModule::set_disable_render_offset_last(agent.module_accessor );
        if custom_effect_guard(agent) {
            EffectModule::set_disable_render_offset_last(agent.module_accessor);
        }
        // EffectModule::enable_sync_init_pos_last(agent.module_accessor);
        let _literal = r#"EffectModule::enable_sync_init_pos_last(agent.module_accessor);"#;
    }

    EffectModule::enable_sync_init_pos_last(agent.module_accessor);
}
"###;
        let parsed = crate::acmd::parse_effect_script(source);
        assert_eq!(parsed.call_macro_ordinals(), vec![0]);
        assert_eq!(
            spawn_site_names(source),
            vec!["EffectModule::set_disable_render_offset_last".to_string()]
        );
        let native_sites = native_effect_control_sites(source);
        assert_eq!(
            native_sites.len(),
            7,
            "comments and string contents are not call sites: {native_sites:#?}"
        );
        let opaque_two_arg = native_sites
            .iter()
            .find(|site| {
                site.name == "EffectModule::enable_sync_init_pos_last" && site.args.len() == 2
            })
            .expect("the opaque two-argument source site is still visible to move safety");
        assert!(all_effect_source_sites(source)
            .iter()
            .any(|site| site.span == opaque_two_arg.span));
        assert_eq!(
            spawn_site_names(source).len(),
            1,
            "nested, malformed, and bare native calls must not consume call ordinals"
        );
    }

    #[test]
    fn fake_execute_headers_cannot_shift_native_control_source_ordinals() {
        let source = r###"unsafe extern "C" fn effect_fake_headers(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_first"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, true);
        /* if macros::is_excute(agent) { */
        let _header_text = r#"if macros::is_excute(agent) { }"#;
        EffectModule::set_disable_render_offset_last(agent.module_accessor);
        /* } */
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_second"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, true);
    }
}
"###;
        let pristine = crate::acmd::parse_effect_script(source).to_effect_calls();
        assert_eq!(pristine.len(), 3);
        assert_eq!(
            spawn_site_names(source),
            [
                "EFFECT_FOLLOW",
                "EffectModule::set_disable_render_offset_last",
                "EFFECT_FOLLOW",
            ]
        );
        let mut edited = pristine.clone();
        edited[2].scale = 1.5;
        let (after, report) =
            rewrite_effect_calls(source, "test/fake_headers", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 1);
        assert!(after.contains("/* if macros::is_excute(agent) { */"));
        assert!(after.contains("r#\"if macros::is_excute(agent) { }\"#"));
        let reparsed = crate::acmd::parse_effect_script(&after).to_effect_calls();
        assert_eq!(reparsed.len(), 3);
        assert_eq!(reparsed[0].scale, 1.0);
        assert_eq!(reparsed[2].scale, 1.5);
    }

    #[test]
    fn native_effect_controls_require_matching_plain_execute_headers() {
        for header in [
            "if macros::is_excute(agent){",
            "if macros::is_excute(agent) && native_guard(agent) {",
            "if !macros::is_excute(agent) {",
        ] {
            let source = format!("unsafe extern \"C\" fn effect_header(agent: &mut L2CAgentBase) {{\n    {header}\n        EffectModule::enable_sync_init_pos_last(agent.module_accessor);\n    }}\n}}\n");
            let parsed = crate::acmd::parse_effect_script(&source);
            assert!(
                parsed.to_effect_calls().is_empty(),
                "noncanonical guard admitted a native control: {source}"
            );
            assert!(
                spawn_site_names(&source).is_empty(),
                "noncanonical guard consumed an ordinal: {source}"
            );
        }
    }

    #[test]
    fn native_effect_ordinals_use_the_importers_scope_and_physical_boundaries() {
        for (body, expected_controls) in [
            ("        if macros::is_excute(agent) {\n            EffectModule::enable_sync_init_pos_last(agent.module_accessor);\n        }", 0),
            ("        let _brace = r#\"{\"#;\n        EffectModule::enable_sync_init_pos_last(agent.module_accessor);", 1),
            ("        let _brace = '\\u{7b}';\n        EffectModule::enable_sync_init_pos_last(agent.module_accessor);", 1),
        ] {
            let source = format!("unsafe extern \"C\" fn effect_scope(agent: &mut L2CAgentBase) {{\n    if macros::is_excute(agent) {{\n{body}\n    }}\n}}\n");
            syn::parse_file(&source).unwrap();
            let parsed = crate::acmd::parse_effect_script(&source);
            let control_count = parsed.to_effect_calls().iter().filter(|call| call.control.is_some()).count();
            assert_eq!(control_count, expected_controls, "only genuine root statements may gain typed native controls: {source}");
            assert_eq!(spawn_site_names(&source).len(), expected_controls, "source scanner must count exactly the imported native controls: {source}");
        }
    }

    #[test]
    fn two_argument_sync_call_stays_raw_and_consumes_no_source_call_ordinal() {
        let source = r#"unsafe extern "C" fn effect_raw_sync_control(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 2.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_first"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, true);
    }
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        EffectModule::enable_sync_init_pos_last(agent.module_accessor, 0);
        EffectModule::set_disable_render_offset_last(boma);
        EffectModule::set_disable_render_offset_last(agent.module_accessor, 0);
    }
    frame(agent.lua_state_agent, 8.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_second"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, true);
    }
}
"#;
        let parsed = crate::acmd::parse_effect_script(source);
        assert_eq!(parsed.call_macro_ordinals(), vec![0, 1]);
        let pristine = parsed.to_effect_calls();
        assert_eq!(pristine.len(), 2);
        assert_eq!(
            spawn_site_names(source),
            vec!["EFFECT_FOLLOW".to_string(), "EFFECT_FOLLOW".to_string()]
        );

        let mut edited = pristine.clone();
        edited[1].offset[0] = 4.0;
        let (after, report) =
            rewrite_effect_calls(source, "test/raw_sync_control", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(
            after.contains("EffectModule::enable_sync_init_pos_last(agent.module_accessor, 0);")
        );
        assert!(after.contains("EffectModule::set_disable_render_offset_last(boma);"));
        assert!(after
            .contains("EffectModule::set_disable_render_offset_last(agent.module_accessor, 0);"));
        assert!(
            after.contains("Hash40::new(\"sys_second\"), Hash40::new(\"top\"), 4.0, 0, 0"),
            "{after}"
        );
        let reparsed = crate::acmd::parse_effect_script(&after).to_effect_calls();
        assert_eq!(reparsed.len(), 2);
        assert_eq!(reparsed[1].offset[0], 4.0);
    }

    #[test]
    fn c4_control_values_write_back_without_reordering_the_effect_script() {
        let source = r#"unsafe extern "C" fn effect_c4(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::EFFECT_DETACH_KIND(agent, Hash40::new("sys_smoke"), 0);
        macros::EFFECT_DETACH_KIND_WORK(agent, *WORK_INT, 0);
        macros::ENABLE_AREA(agent, 2);
        macros::UNABLE_AREA(agent, 2);
    }

"#;
        let pristine = crate::acmd::parse_effect_script(source).to_effect_calls();
        assert_eq!(pristine.len(), 4);
        let mut edited = pristine.clone();
        edited[0].control = Some(crate::data::EffectControl::DetachKind {
            effect_name: "sys_smoke_alt".into(),
            unk: -1,
        });
        edited[1].control = Some(crate::data::EffectControl::DetachKindWork {
            work: "WORK_INT_ALT".into(),
            unk: -2,
        });
        edited[2].control = Some(crate::data::EffectControl::EnableArea { kind: "3".into() });
        edited[3].control = Some(crate::data::EffectControl::UnableArea { kind: "4".into() });

        let (after, report) = rewrite_effect_calls(source, "test/c4", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(
            report.changed, 6,
            "each edited C4 value is one source replacement"
        );
        assert!(after.contains("EFFECT_DETACH_KIND(agent, Hash40::new(\"sys_smoke_alt\"), -1);"));
        assert!(after.contains("EFFECT_DETACH_KIND_WORK(agent, *WORK_INT_ALT, -2);"));
        assert!(after.contains("ENABLE_AREA(agent, 3);"));
        assert!(after.contains("UNABLE_AREA(agent, 4);"));
        assert_eq!(
            crate::acmd::parse_effect_script(&after).to_effect_calls(),
            edited,
            "source write-back must parse back to the edited control values"
        );
    }

    #[test]
    fn off_kind_control_keeps_following_effect_source_ordinal_aligned() {
        let source = r#"unsafe extern "C" fn effect_control_ordinal(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::EFFECT_OFF_KIND_WORK(agent, *FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_EFFECT_KIND_SWORD_FLARE, false, true);
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_hit_elec"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, true);
    }
}
"#;
        let parsed = crate::acmd::parse_effect_script(source);
        assert_eq!(parsed.call_macro_ordinals(), vec![0, 1]);
        let pristine = parsed.to_effect_calls();
        assert_eq!(pristine.len(), 2);
        let mut edited = pristine.clone();
        let Some(crate::data::EffectControl::OffKindWork { preset_id }) =
            edited[0].control.as_mut()
        else {
            panic!("first source call should be the typed off-kind control");
        };
        *preset_id = "cloud_sword_flare_tt".into();
        edited[1].offset[0] = 3.5;

        let (after, report) =
            rewrite_effect_calls(source, "test/off_kind_ordinal", &pristine, &edited).unwrap();
        assert_eq!(
            report.skipped.len(),
            1,
            "closed preset writes should fail closed, while the next independent effect edit remains writable: {report:?}"
        );
        assert!(report.skipped[0].contains("closed slot/flag tuples"));
        assert!(after.contains("EFFECT_OFF_KIND_WORK(agent, *FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_EFFECT_KIND_SWORD_FLARE, false, true);"));
        assert!(
            after.contains("Hash40::new(\"sys_hit_elec\"), Hash40::new(\"top\"), 3.5, 0, 0"),
            "{after}"
        );
        let reparsed = crate::acmd::parse_effect_script(&after).to_effect_calls();
        assert_eq!(reparsed.len(), 2);
        assert!(
            matches!(reparsed[0].control.as_ref(), Some(crate::data::EffectControl::OffKindWork { preset_id }) if preset_id == "cloud_sword_flare_ft")
        );
        assert_eq!(reparsed[1].offset[0], 3.5);
    }

    /// The source editor and the editor panels drive each other, so a value written into the
    /// text has to parse back to exactly what was written. Any drift — a rounding difference,
    /// a reformat — reads as a fresh edit on the next frame and the two ping-pong forever.
    #[test]
    fn a_panel_edit_written_to_source_parses_back_unchanged() {
        let (_tmp, index) = mario_project();
        let body = index.script_source("mario", "attack_air_n").unwrap().body;
        let effect_fn = body[body.find("unsafe extern \"C\" fn effect_").unwrap()..].to_string();
        let pristine = crate::acmd::parse_effect_script(&effect_fn).to_effect_calls();

        let mut edited = pristine.clone();
        edited[0].offset = [1.0 / 3.0, -0.125, 12.5];
        edited[0].rotation = [90.0, 0.5, -45.0];
        edited[0].scale = 0.7;

        let (updated, report) =
            rewrite_effect_calls(&effect_fn, "mario/attack_air_n", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");

        let reparsed = crate::acmd::parse_effect_script(&updated).to_effect_calls();
        assert_eq!(reparsed.len(), 1);
        for (axis, (got, want)) in reparsed[0]
            .offset
            .iter()
            .chain(&reparsed[0].rotation)
            .zip(edited[0].offset.iter().chain(&edited[0].rotation))
            .enumerate()
        {
            assert!(
                (got - want).abs() < 1e-4,
                "axis {axis}: wrote {want}, read back {got}"
            );
        }
        assert!((reparsed[0].scale - 0.7).abs() < 1e-4);

        // And the second pass is a no-op: this is what actually breaks the feedback loop.
        let (again, report) =
            rewrite_effect_calls(&updated, "mario/attack_air_n", &reparsed, &edited).unwrap();
        assert_eq!(report.changed, 0, "{report:?}");
        assert_eq!(again, updated, "a settled buffer must be byte-identical");
    }

    /// Same convergence property for the hitbox side.
    #[test]
    fn a_hitbox_edit_written_to_source_parses_back_unchanged() {
        let (_tmp, index) = mario_project();
        let body = index.script_source("mario", "attack_air_n").unwrap().body;
        let game_fn = body[..body.find("unsafe extern \"C\" fn effect_").unwrap()].to_string();
        let pristine = crate::acmd::parse_acmd_script(&game_fn).to_hitboxes();

        let mut edited = pristine.clone();
        edited[0].damage = 13.7;
        edited[0].angle = 270;
        edited[0].kb_base = 62;
        edited[0].offset_y = -3.25;
        edited[0].hitlag_mult = 1.5;

        let (updated, report) =
            rewrite_hitboxes(&game_fn, "mario/attack_air_n", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");

        let reparsed = crate::acmd::parse_acmd_script(&updated).to_hitboxes();
        assert_eq!(reparsed[0].damage, 13.7);
        assert_eq!(reparsed[0].angle, 270);
        assert_eq!(reparsed[0].kb_base, 62);
        assert_eq!(reparsed[0].offset_y, -3.25);
        assert_eq!(reparsed[0].hitlag_mult, 1.5);

        let (again, report) =
            rewrite_hitboxes(&updated, "mario/attack_air_n", &reparsed, &edited).unwrap();
        assert_eq!(report.changed, 0, "{report:?}");
        assert_eq!(again, updated);
    }

    #[test]
    fn repeated_hitbox_ids_use_their_source_ordinal_when_frames_differ() {
        let text = r#"unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("top"), 10.0, 361, 100, 0, 40, 4.0, 0.0, 8.0, 0.0, None, None, None, 1.0, 1.0, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_POS, false, 0, 0.0, 0, false, false, false, false, false, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_M, *COLLISION_SOUND_ATTR_PUNCH, *ATTACK_REGION_PUNCH);
    }
    frame(agent.lua_state_agent, 15.0);
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("top"), 12.0, 361, 100, 0, 40, 4.0, 0.0, 8.0, 0.0, None, None, None, 1.0, 1.0, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_POS, false, 0, 0.0, 0, false, false, false, false, false, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_M, *COLLISION_SOUND_ATTR_PUNCH, *ATTACK_REGION_PUNCH);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_hitboxes();
        assert_eq!(
            pristine.len(),
            2,
            "both reuses must remain separate hitboxes"
        );
        assert_eq!(pristine[0].active_start, 5);
        assert_eq!(pristine[1].active_start, 15);

        let mut edited = pristine.clone();
        edited[1].damage = 18.0;
        let (after, report) = rewrite_hitboxes(text, "test", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 1, "only the second source call changed");
        let reparsed = crate::acmd::parse_acmd_script(&after).to_hitboxes();
        assert_eq!(reparsed[0].damage, 10.0, "the first reuse is untouched");
        assert_eq!(
            reparsed[1].damage, 18.0,
            "the later reuse receives the edit"
        );
    }

    #[test]
    fn source_sync_relocates_a_script_after_external_bytes_shift_its_span() {
        let (tmp, index) = mario_project();
        let site = index.script("mario", "effect_attackairn").unwrap().clone();
        let original = std::fs::read_to_string(&site.file).unwrap();
        let original_body = &original[site.span.clone()];
        let pristine = crate::acmd::parse_effect_script(original_body).to_effect_calls();
        let mut edited = pristine.clone();
        edited[0].scale = 7.25;

        // This is the external/source-generation edit that leaves the caller's old snapshot
        // with an in-bounds but stale span. The write must follow the function identity, not the
        // old byte offset.
        std::fs::write(&site.file, format!("// generated header\n{original}")).unwrap();
        let report =
            sync_effect_calls(&index, "mario", "attack_air_n", &pristine, &edited).unwrap();
        assert_eq!(
            report.changed, 1,
            "the relocated effect still gets its edit"
        );

        let fresh = SourceIndex::build(tmp.path()).unwrap();
        let current = fresh.script("mario", "effect_attackairn").unwrap();
        let source = std::fs::read_to_string(&current.file).unwrap();
        let body = &source[current.span.clone()];
        let reparsed = crate::acmd::parse_effect_script(body).to_effect_calls();
        assert_eq!(
            reparsed[0].scale, 7.25,
            "the current effect function was edited"
        );
        assert!(
            source.contains("fn game_attackairn"),
            "the sibling script survived"
        );
        assert!(source.starts_with("// generated header\n"));
    }

    #[test]
    fn syncing_refuses_to_invent_structure() {
        let (tmp, index) = mario_project();
        let before = std::fs::read_to_string(tmp.path().join("src/mario/acmd.rs")).unwrap();
        let body = index.script_source("mario", "attack_air_n").unwrap().body;
        let pristine = crate::acmd::parse_effect_script(&body).to_effect_calls();

        // Renaming the graphic is not a value change to this line.
        let mut edited = pristine.clone();
        edited[0].effect_name = "sys_something_else".into();
        edited[0].scale = 3.0;
        let report =
            sync_effect_calls(&index, "mario", "attack_air_n", &pristine, &edited).unwrap();
        assert_eq!(report.skipped.len(), 1, "{report:?}");

        // Changing the spawn family is structural as well: the target may have a different
        // number/type of tail arguments and may add or remove the flip/follow slots. Export and
        // live replay support it, but source sync must report it rather than rewrite by position.
        let mut edited = pristine.clone();
        edited[0].spawn_func = "EFFECT_FOLLOW".into();
        edited[0].effect_name_alt = None;
        edited[0].follows_bone = true;
        edited[0].active_end = 9999;
        let report =
            sync_effect_calls(&index, "mario", "attack_air_n", &pristine, &edited).unwrap();
        assert!(
            report
                .skipped
                .iter()
                .any(|message| message.contains("spawn command")),
            "{report:?}"
        );

        // The two refusals above change no value, so the file is byte-identical.
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("src/mario/acmd.rs")).unwrap(),
            before
        );

        // Adding a call inserts it as a new frame-block entry (issue 35): a new
        // effect the script never had is the ordinary "import a new effect"
        // case, and refusing it left the source silently behind the editor.
        let mut edited = pristine.clone();
        edited.push(pristine[0].clone());
        let report =
            sync_effect_calls(&index, "mario", "attack_air_n", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(report.changed > 0, "{report:?}");
        assert_ne!(
            std::fs::read_to_string(tmp.path().join("src/mario/acmd.rs")).unwrap(),
            before,
            "an added spawn must reach the source file"
        );
    }

    #[test]
    fn adding_an_effect_call_inserts_it_into_source() {
        let (_tmp, index) = mario_project();
        let body = index.script_source("mario", "attack_air_n").unwrap().body;
        let pristine = crate::acmd::parse_effect_script(&body).to_effect_calls();
        let mut added = pristine[0].clone();
        added.effect_name = "sys_hit_elec".into();
        added.bone_name = "top".into();
        added.active_start = 99;
        added.normalize_timing();
        let mut edited = pristine.clone();
        edited.push(added.clone());
        let (updated, report) =
            rewrite_effect_calls(&body, "mario/attack_air_n", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(report.changed > 0, "{report:?}");
        let reparsed = crate::acmd::parse_effect_script(&updated).to_effect_calls();
        assert_eq!(
            reparsed.len(),
            edited.len(),
            "inserted spawn must parse back: {updated}"
        );
        let last = reparsed.last().unwrap();
        assert_eq!(last.effect_name, "sys_hit_elec");
        assert_eq!(last.active_start, 99);
    }

    #[test]
    fn removing_an_effect_call_is_reported_instead_of_claiming_source_sync() {
        let (_tmp, index) = mario_project();
        let body = index.script_source("mario", "attack_air_n").unwrap().body;
        let pristine = crate::acmd::parse_effect_script(&body).to_effect_calls();
        let (updated, report) =
            rewrite_effect_calls(&body, "mario/attack_air_n", &pristine, &[]).unwrap();
        assert_eq!(
            updated, body,
            "source sync cannot delete the authored spawn"
        );
        assert_eq!(report.changed, 0, "a refused removal writes nothing");
        assert!(
            report
                .skipped
                .iter()
                .any(|message| message.contains("removed")),
            "the UI must explain why the source still has the call: {report:?}"
        );
    }

    #[test]
    fn removing_first_same_named_effect_never_retargets_the_next_spawn() {
        let source = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_same"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, false);
    }
    frame(agent.lua_state_agent, 10.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_same"), Hash40::new("top"), 1, 2, 3, 0, 0, 0, 2, false);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(source).to_effect_calls();
        assert_eq!(pristine.len(), 2);
        assert_ne!(pristine[0].offset, pristine[1].offset);
        assert_ne!(pristine[0].scale, pristine[1].scale);

        // Removing the first call shifts the second call into edited[0]. Without a proven
        // removal map, that value would be mistaken for an edit to the first source macro.
        let edited = vec![pristine[1].clone()];
        let (updated, report) = rewrite_effect_calls(source, "test", &pristine, &edited).unwrap();
        assert_eq!(
            updated, source,
            "a removal must not retune a surviving same-named spawn"
        );
        assert_eq!(report.changed, 0, "refused removal must not write values");
        assert!(
            report
                .skipped
                .iter()
                .any(|message| message.contains("removed")),
            "the refusal must be visible: {report:?}"
        );
    }

    #[test]
    fn adding_an_effect_inside_a_conditional_is_refused_without_guessing_its_arm() {
        let text = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        if WorkModule::is_flag(agent.module_accessor, *FLAG) {
            macros::EFFECT(agent, Hash40::new("sys_hit"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, false);
        }
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(text).to_effect_calls();
        assert_eq!(pristine.len(), 1);
        let mut added = pristine[0].clone();
        added.effect_name = "sys_added".into();
        added.active_start = 9;
        added.normalize_timing();
        let mut edited = pristine.clone();
        edited.push(added);

        let (updated, report) = rewrite_effect_calls(text, "test", &pristine, &edited).unwrap();
        assert_eq!(
            updated, text,
            "an ambiguous conditional must remain untouched"
        );
        assert!(
            report
                .skipped
                .iter()
                .any(|message| message.contains("execution context")),
            "the refusal must be visible: {report:?}"
        );
    }

    /// A spawn added between two `wait` points lands on its own frame: the wait is split around
    /// it, so the later spawn still runs on the frame it did.
    #[test]
    fn adding_an_effect_between_wait_points_splits_the_wait() {
        let text = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_first"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, false);
    }
    wait(agent.lua_state_agent, 2.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_second"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, false);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(text).to_effect_calls();
        assert_eq!(pristine.len(), 2);
        assert_eq!(pristine[1].active_start, 7);
        let mut added = pristine[0].clone();
        added.effect_name = "sys_between".into();
        added.active_start = 6;
        added.normalize_timing();
        let mut edited = pristine.clone();
        edited.push(added);

        let (updated, report) = rewrite_effect_calls(text, "test", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}
{updated}");
        assert_eq!(updated.matches("wait(agent.lua_state_agent, 1.0);").count(), 2, "{updated}");
        let calls = crate::acmd::parse_effect_script(&updated).to_effect_calls();
        let frame_of = |name: &str| calls.iter().find(|c| c.effect_name == name).map(|c| c.active_start);
        assert_eq!(frame_of("sys_first"), Some(5));
        assert_eq!(frame_of("sys_between"), Some(6));
        assert_eq!(frame_of("sys_second"), Some(7));
    }

    #[test]
    fn a_loop_body_only_syncs_when_every_iteration_agrees() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "src/lib.rs",
            r#"
unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    for _ in 0..3 {
        wait(agent.lua_state_agent, 2.0);
        if macros::is_excute(agent) {
            macros::EFFECT(agent, Hash40::new("g"), Hash40::new("top"), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0, 0, 0, 0, 0, 0, false);
        }
    }
}
pub fn install() { let agent = &mut smashline::Agent::new("test_fighter"); }
"#,
        );
        let index = SourceIndex::build(tmp.path()).unwrap();
        let body = index.script_source("test_fighter", "test").unwrap().body;
        let pristine = crate::acmd::parse_effect_script(&body).to_effect_calls();
        assert_eq!(pristine.len(), 3, "one line, three iterations");

        // One iteration alone cannot be expressed in a single line of source.
        let mut edited = pristine.clone();
        edited[1].scale = 2.0;
        let report = sync_effect_calls(&index, "test_fighter", "test", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 0);
        assert_eq!(report.skipped.len(), 1, "{report:?}");

        // All three together are just a value change to that line.
        let mut edited = pristine.clone();
        for call in &mut edited {
            call.scale = 2.0;
        }
        let report = sync_effect_calls(&index, "test_fighter", "test", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(std::fs::read_to_string(tmp.path().join("src/lib.rs"))
            .unwrap()
            .contains("0.0, 0.0, 0.0, 2.0, 0"));
    }

    #[test]
    fn syncing_a_hitbox_retunes_its_attack_arguments() {
        let (tmp, index) = mario_project();
        let body = index.script_source("mario", "attack_air_n").unwrap().body;
        let pristine = crate::acmd::parse_acmd_script(&body).to_hitboxes();
        assert_eq!(pristine.len(), 1);

        let mut edited = pristine.clone();
        edited[0].damage = 12.5;
        edited[0].angle = 45;
        edited[0].size = 5.0;

        let report = sync_hitboxes(&index, "mario", "attack_air_n", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 3, "{report:?}");

        let after = std::fs::read_to_string(tmp.path().join("src/mario/acmd.rs")).unwrap();
        assert!(
            after.contains(
                r#"macros::ATTACK(agent, 0, 0, Hash40::new("top"), 12.5, 45, 100, 0, 40, 5.0, 0.0, 8.0, 6.0, None,"#
            ),
            "{after}"
        );
        // The const-valued arguments after the numbers are untouched.
        assert!(after.contains("*ATTACK_SETOFF_KIND_ON"), "{after}");
    }

    /// Effect-first files share one file for both categories, so an effect write that
    /// changes the byte length shifts the game function's span. A game sync reusing the
    /// pre-write index then reads the wrong body — the shape issue 37 reports as hitbox
    /// edits never reaching the game script while effect edits land. The caller
    /// (`sync_edits_to_source`) must re-index between the two passes; this pins that
    /// the refreshed sequence lands both edits.
    #[test]
    fn effect_then_hitbox_syncs_share_one_file_effect_first() {
        let tmp = tempfile::tempdir().unwrap();
        // Same two functions as MARIO, but effect first: alphabetical layouts put
        // `effect_` before `game_`, so an effect write moves the game span.
        let body = r#"
use smash::{lua2cpp::*, phx::*};

unsafe extern "C" fn effect_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_FLIP(agent, Hash40::new("sys_hit_l"), Hash40::new("sys_hit_r"), Hash40::new("haver"), 1.0, 2.0, 3.0, 0.0, 90.0, 45.0, 1.5, true, *EF_FLIP_YZ);
    }
}

unsafe extern "C" fn game_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("top"), 8.0, 361, 100, 0, 40, 4.5, 0.0, 8.0, 6.0, None, None, None, 1.0, 1.0, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_POS, false, 0, 0.0, 0, false, false, false, false, false, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_M, *COLLISION_SOUND_ATTR_PUNCH, *ATTACK_REGION_PUNCH);
    }
}

pub fn install(agent: &mut smashline::Agent) {
    agent.acmd("game_attackairn", game_attackairn, smashline::Priority::Default);
    agent.acmd("effect_attackairn", effect_attackairn, smashline::Priority::Default);
}
"#;
        write(tmp.path(), "src/mario/acmd.rs", body);
        write(
            tmp.path(),
            "src/mario/mod.rs",
            "pub fn install() { let agent = &mut smashline::Agent::new(\"mario\"); }",
        );
        let index = SourceIndex::build(tmp.path()).unwrap();
        let merged = index.script_source("mario", "attack_air_n").unwrap().body;
        let hit_pristine = crate::acmd::parse_acmd_script(&merged).to_hitboxes();
        let eff_pristine = crate::acmd::parse_effect_script(&merged).to_effect_calls();
        assert_eq!(hit_pristine.len(), 1);
        assert_eq!(eff_pristine.len(), 1);

        let mut hit_edited = hit_pristine.clone();
        hit_edited[0].damage = 12.5;
        let mut eff_edited = eff_pristine.clone();
        // 1.5 -> 12.5 grows the effect function by one byte, shifting the game span.
        eff_edited[0].scale = 12.5;

        // Effect pass first, exactly as `sync_edits_to_source` orders it.
        let effect_report =
            sync_effect_calls(&index, "mario", "attack_air_n", &eff_pristine, &eff_edited).unwrap();
        assert!(effect_report.changed > 0, "{effect_report:?}");

        // Re-index before the game pass — without this the stale game span points into
        // the wrong bytes and the hitbox write misses. This is the refresh the caller
        // must do; the test fails if the two passes share one stale index.
        let fresh = SourceIndex::build(tmp.path()).unwrap();
        let hit_report =
            sync_hitboxes(&fresh, "mario", "attack_air_n", &hit_pristine, &hit_edited).unwrap();
        assert_eq!(hit_report.changed, 1, "{hit_report:?}");

        let after = std::fs::read_to_string(tmp.path().join("src/mario/acmd.rs")).unwrap();
        assert!(
            after.contains(r#"macros::ATTACK(agent, 0, 0, Hash40::new("top"), 12.5, 361,"#),
            "hitbox damage must land: {after}"
        );
        assert!(
            !after.contains("0.0, 90.0, 45.0, 1.5, true"),
            "old effect scale must be gone: {after}"
        );
        assert!(
            after.contains("0.0, 90.0, 45.0, 12.5, true"),
            "effect scale must land: {after}"
        );
    }

    /// A wrapper-form trail, in the shape `smash-script` actually declares.
    ///
    /// Hand-written, because no corpus script calls this spelling — the four vanilla trails are
    /// the raw `effect(*MA_MSC_CMD_EFFECT_AFTER_IMAGE3_ON, …)` form. So its *layout* is taken
    /// from the declaration rather than invented: 29 arguments, `trail_bone1` at 4 with
    /// `trail_x1/y1/z1` at 5..=7, `trail_bone2` at 8. The version this replaced put `sword2` at
    /// slot 5 and `0.75` at slot 8 — a `Hash40` where a coordinate goes and a float where the
    /// second joint goes — which was harmless only for as long as nothing read slot 8. It now
    /// does, and a fixture whose shape is made up would have had the editor offering `0.75` as
    /// an editable joint.
    const TRAIL: &str = r#"unsafe extern "C" fn effect_attacks4(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::AFTER_IMAGE4_ON_arg29(agent, Hash40::new("tex1"), Hash40::new("tex2"), 4, Hash40::new("sword1"), 0, 3, 0.25, Hash40::new("sword2"), 0, 26, 0.5, true, Hash40::new("null"), Hash40::new("haver"), 0, 0, 0, 0, 0, 0, 1, 0, *EFFECT_AXIS_X, 0, *TRAIL_BLEND_BLEND_SRC_ONE, 1, 0, 0.0, 0.0);
    }
}
"#;

    /// A trail's arguments are textures and trail parameters, and slots 5..=7 are its first
    /// edge's offset, NOT the spawn transform. Writing the spawn layout into them replaced the
    /// trail's own parameters with position values and reported success.
    #[test]
    fn a_trail_is_never_written_to_through_the_spawn_transform_layout() {
        let pristine = crate::acmd::parse_effect_script(TRAIL).to_effect_calls();
        assert_eq!(pristine.len(), 1, "{pristine:#?}");
        let mut edited = pristine.clone();
        edited[0].offset = [2.5, 0.0, 0.0];
        edited[0].scale = 3.0;

        let (after, report) = rewrite_effect_calls(TRAIL, "t", &pristine, &edited).unwrap();
        assert_eq!(after, TRAIL, "a trail call must come back untouched");
        assert_eq!(report.changed, 0);
        assert_eq!(report.skipped.len(), 1, "{report:?}");
        assert!(report.skipped[0].contains("no position"), "{report:?}");
    }

    /// The declared wrapper form yields both joints, from the slots the declaration names.
    ///
    /// This is the fixture's own guard: it reads the two joints out of positions holding
    /// *different* values, so a parser taking `trail_bone2` from slot 4, or `trail_bone1` from
    /// slot 8, fails here. On any vanilla call the two agree and no such test is possible.
    #[test]
    fn the_declared_wrapper_trail_yields_both_of_its_joints() {
        let calls = crate::acmd::parse_effect_script(TRAIL).to_effect_calls();
        assert_eq!(calls[0].bone_name, "sword1");
        assert_eq!(calls[0].trail_bone2.as_deref(), Some("sword2"));
    }

    /// Moving only the second joint is reported as the edit it is.
    ///
    /// **This pins the wording, and that is the whole of what it pins.** A trail meets two
    /// guards in sequence — `identity_matches` first, the "no transform to write" check second —
    /// and either alone leaves the source untouched, so nothing about *safety* distinguishes
    /// them. What differs is the sentence the user gets after editing `Bone 2` and syncing:
    /// "you changed a joint, and syncing only rewrites transform values", or "this call has no
    /// transform", which is true of the call and says nothing about their edit.
    ///
    /// Asserting on the substring `joint` does not separate those — both messages contain it,
    /// and dropping `trail_bone2` from `identity_matches` survived a version of this test that
    /// did. Whether the edit is *seen* at all is covered elsewhere: `differs` is a plain `!=`
    /// over the whole call and catches it either way.
    #[test]
    fn moving_only_the_second_joint_is_reported_as_a_change_source_syncing_will_not_make() {
        let pristine = crate::acmd::parse_effect_script(TRAIL).to_effect_calls();
        let mut edited = pristine.clone();
        edited[0].trail_bone2 = Some("blade".into());

        let (after, report) = rewrite_effect_calls(TRAIL, "t", &pristine, &edited).unwrap();
        assert_eq!(after, TRAIL, "a trail call must come back untouched");
        assert_eq!(report.changed, 0);
        assert_eq!(report.skipped.len(), 1, "{report:?}");
        assert!(
            report.skipped[0]
                .contains("changed graphic, joint, spawn command, timing, or enablement"),
            "the report must name what the user changed, not the transform this call never \
             had: {report:?}"
        );
    }

    /// Kirby's down attack, verbatim: two spawns, each with its own rate, in one block. The
    /// rate macro names no effect, so the only thing saying which spawn a rate belongs to is
    /// that it sits directly beneath it — and getting that wrong writes one spawn's value
    /// into the other's line.
    const RATES: &str = r#"unsafe extern "C" fn effect_downattackd(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 15.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_atk_smoke"), Hash40::new("top"), 0, 0, 0, 0, 180, 0, 0.4, 0, 0, 0, 0, 0, 0, false);
        macros::LAST_EFFECT_SET_RATE(agent, 2);
        macros::EFFECT_FOLLOW_FLIP(agent, Hash40::new("sys_attack_line"), Hash40::new("sys_attack_line"), Hash40::new("top"), -8, 4, 2.5, 0, 160, 0, 1.1, true, *EF_FLIP_YZ);
        macros::LAST_EFFECT_SET_RATE(agent, 1.5);
    }
}
"#;

    #[test]
    fn a_rate_edit_rewrites_only_its_own_spawns_rate_line() {
        let pristine = crate::acmd::parse_effect_script(RATES).to_effect_calls();
        assert_eq!(
            pristine.iter().map(|c| c.rate).collect::<Vec<_>>(),
            vec![Some(2.0), Some(1.5)]
        );

        let mut edited = pristine.clone();
        edited[1].rate = Some(0.75);
        let (after, report) = rewrite_effect_calls(RATES, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 1);
        assert!(
            after.contains("macros::LAST_EFFECT_SET_RATE(agent, 2);"),
            "the first spawn's rate must be untouched:\n{after}"
        );
        assert!(
            after.contains("macros::LAST_EFFECT_SET_RATE(agent, 0.75);"),
            "the second spawn's rate is the one that changed:\n{after}"
        );
        // A whole number stays whole — the slot is generic over `ToF32`, and the emitter
        // spells it the same way, so the two export paths agree on the text.
        let mut whole = pristine.clone();
        whole[0].rate = Some(3.0);
        let (after, _) = rewrite_effect_calls(RATES, "t", &pristine, &whole).unwrap();
        assert!(
            after.contains("macros::LAST_EFFECT_SET_RATE(agent, 3);"),
            "a whole rate must not sprout a decimal point:\n{after}"
        );
    }

    /// The rate lives on a line of its own, so switching it on or off adds or deletes a call.
    /// That is structural, and the house rule is to name it rather than guess where the line
    /// should go — or, worse, delete one of the user's.
    #[test]
    fn turning_a_rate_off_is_reported_rather_than_deleting_the_line() {
        let pristine = crate::acmd::parse_effect_script(RATES).to_effect_calls();
        let mut edited = pristine.clone();
        edited[0].rate = None;

        let (after, report) = rewrite_effect_calls(RATES, "t", &pristine, &edited).unwrap();
        assert_eq!(after, RATES, "the user's line must still be there");
        assert!(
            report.skipped.iter().any(|s| s.contains("lost its rate")),
            "{report:?}"
        );
    }

    #[test]
    fn turning_a_rate_on_is_reported_rather_than_inventing_a_line() {
        // One spawn with no rate at all, so there is nowhere for a value to be written.
        let text = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_attack"), Hash40::new("top"), 0, 1, 0, 0, 0, 0, 1, true);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(text).to_effect_calls();
        assert_eq!(pristine[0].rate, None);
        let mut edited = pristine.clone();
        edited[0].rate = Some(1.25);

        let (after, report) = rewrite_effect_calls(text, "t", &pristine, &edited).unwrap();
        assert_eq!(
            after, text,
            "nothing may be inserted into the user's script"
        );
        assert!(
            report.skipped.iter().any(|s| s.contains("gained a rate")),
            "{report:?}"
        );
    }

    const CAMERA_OFFSETS: &str = r#"unsafe extern "C" fn effect_camera(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_hit"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, true);
        macros::LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT(agent, -5);
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_hit_2"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, true);
        macros::LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT(agent, 0.4);
    }
}
"#;

    #[test]
    fn a_camera_flat_offset_edit_rewrites_only_its_own_modifier_line() {
        let pristine = crate::acmd::parse_effect_script(CAMERA_OFFSETS).to_effect_calls();
        assert_eq!(
            pristine.iter().map(|c| c.camera_offset).collect::<Vec<_>>(),
            vec![Some(-5.0), Some(0.4)]
        );
        let mut edited = pristine.clone();
        edited[1].camera_offset = Some(0.75);

        let (after, report) = rewrite_effect_calls(CAMERA_OFFSETS, "t", &pristine, &edited)
            .expect("camera-flat offset source rewrite");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 1);
        assert!(
            after.contains("macros::LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT(agent, -5);"),
            "the first spawn's modifier must be untouched:\n{after}"
        );
        assert!(
            after.contains("macros::LAST_EFFECT_SET_OFFSET_TO_CAMERA_FLAT(agent, 0.75);"),
            "the second spawn's modifier must be retuned:\n{after}"
        );
    }

    #[test]
    fn a_particle_tint_edit_rewrites_only_its_particle_modifier_line() {
        const PARTICLE: &str = r#"unsafe extern "C" fn effect_particle(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 7.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_hit"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, true);
        macros::LAST_PARTICLE_SET_COLOR(agent, 0.1, 1.2, 0.3);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(PARTICLE).to_effect_calls();
        assert_eq!(pristine[0].particle_tint, Some([0.1, 1.2, 0.3]));
        let mut edited = pristine.clone();
        edited[0].particle_tint = Some([0.1, 0.5, 0.3]);
        let (after, report) = rewrite_effect_calls(PARTICLE, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 1);
        assert!(after.contains("macros::LAST_PARTICLE_SET_COLOR(agent, 0.1, 0.5, 0.3);"));
        assert!(!after.contains("LAST_EFFECT_SET_COLOR"));
    }

    #[test]
    fn a_dynamic_scale_w_edit_rewrites_only_its_native_value_list() {
        const SCALE_W: &str = r#"unsafe extern "C" fn effect_scale_w(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 7.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_hit"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, true);
        macros::LAST_EFFECT_SET_SCALE_W(agent, 1, 2);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(SCALE_W).to_effect_calls();
        assert_eq!(pristine[0].scale_w, Some(vec![1.0, 2.0]));
        let mut edited = pristine.clone();
        edited[0].scale_w = Some(vec![1.0, 3.5]);

        let (after, report) = rewrite_effect_calls(SCALE_W, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 1);
        assert!(after.contains("macros::LAST_EFFECT_SET_SCALE_W(agent, 1, 3.5);"));
    }

    /// Two spawns, each with a different mix of the three modifiers, and a rate written *after*
    /// a tint on the same spawn — the case that decides whether a modifier line ends the run for
    /// the modifiers after it. It must not: both lines name the spawn above the pair.
    const MODIFIERS: &str = r#"unsafe extern "C" fn effect_attackairhi(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 10.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_FLIP(agent, Hash40::new("kirby_attack_arc"), Hash40::new("kirby_attack_arc"), Hash40::new("top"), -3, 7, 0, 0, 90, 90, 1, true, *EF_FLIP_YZ);
        macros::LAST_EFFECT_SET_COLOR(agent, 0.25, 1.3, 2.5);
        macros::LAST_EFFECT_SET_RATE(agent, 2);
        macros::EFFECT(agent, Hash40::new("sys_attack_impact"), Hash40::new("top"), 13, 6, 1, 0, 0, 0, 1, 1, 1, 1, 0, 0, 360, false);
        macros::LAST_EFFECT_SET_ALPHA(agent, 0.7);
    }
}
"#;

    /// dolly/SpecialAirHiCommand's hurtbox lines, plus the `COL_PRI` pair, in one function.
    /// Two bones set and taken back means four `HIT_NODE` calls sharing two argument shapes —
    /// which is what makes writing to the wrong site visible instead of coincidentally right.
    const HURTBOXES: &str = r#"unsafe extern "C" fn game_specialairhicommand(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 9.0);
    if macros::is_excute(agent) {
        macros::HIT_NODE(agent, Hash40::new("kneer"), *HIT_STATUS_XLU);
        macros::HIT_NODE(agent, Hash40::new("legl"), *HIT_STATUS_XLU);
        macros::COL_PRI(agent, 200);
    }
    frame(agent.lua_state_agent, 20.0);
    if macros::is_excute(agent) {
        macros::HIT_NODE(agent, Hash40::new("kneer"), *HIT_STATUS_NORMAL);
        macros::HIT_NODE(agent, Hash40::new("legl"), *HIT_STATUS_NORMAL);
        macros::COL_NORMAL(agent);
    }
}
"#;

    fn hurt_of(
        text: &str,
    ) -> (
        Vec<crate::data::HurtboxState>,
        Vec<crate::data::ColPriState>,
    ) {
        crate::acmd::parse_acmd_script(text).to_hurtboxes()
    }

    /// kirby/ThrowF verbatim — two `ATTACK_ABS` sharing id 0, told apart only by kind.
    const THROW_ABS: &str = r#"unsafe extern "C" fn game_throwf(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::ATTACK_ABS(agent, *FIGHTER_ATTACK_ABSOLUTE_KIND_THROW, 0, 5.0, 75, 125, 0, 40, 0.0, 1.0, *ATTACK_LR_CHECK_F, 0.0, true, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_NONE, *ATTACK_REGION_THROW);
        macros::ATTACK_ABS(agent, *FIGHTER_ATTACK_ABSOLUTE_KIND_CATCH, 0, 3.0, 361, 100, 0, 60, 0.0, 1.0, *ATTACK_LR_CHECK_F, 0.0, true, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_NONE, *ATTACK_REGION_THROW);
    }
}
"#;

    /// Both calls carry id 0, so the sync has to match on kind. Getting that wrong writes the
    /// throw's damage into the catch line, which compiles and is wrong.
    #[test]
    fn an_absolute_hit_is_retuned_by_kind_rather_than_by_its_shared_id() {
        let pristine = crate::acmd::parse_acmd_script(THROW_ABS).to_hitboxes();
        assert_eq!(pristine.len(), 2);
        let mut edited = pristine.clone();
        edited[1].damage = 4.5;

        let (after, report) = rewrite_hitboxes(THROW_ABS, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 1, "one argument span");
        assert!(
            after.contains("*FIGHTER_ATTACK_ABSOLUTE_KIND_CATCH, 0, 4.5, 361,"),
            "the catch line takes the edit:\n{after}"
        );
        assert!(
            after.contains("*FIGHTER_ATTACK_ABSOLUTE_KIND_THROW, 0, 5.0, 75,"),
            "the throw line beside it must be untouched:\n{after}"
        );
    }

    /// The slot order is not `ATTACK`'s. Writing through that table would land each value one
    /// or more places off, so every editable slot is checked against its own position.
    #[test]
    fn every_editable_absolute_slot_lands_in_its_own_position() {
        let pristine = crate::acmd::parse_acmd_script(THROW_ABS).to_hitboxes();
        let mut edited = pristine.clone();
        let hb = &mut edited[0];
        hb.damage = 6.5;
        hb.angle = 45;
        hb.kb_scaling = 130;
        hb.fkb = 7;
        hb.kb_base = 55;
        hb.hitlag_mult = 0.5;
        hb.sound_level = "ATTACK_SOUND_LEVEL_L".into();
        hb.attack_region = "ATTACK_REGION_NONE".into();

        let (after, report) = rewrite_hitboxes(THROW_ABS, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(
            after.contains(
                "*FIGHTER_ATTACK_ABSOLUTE_KIND_THROW, 0, 6.5, 45, 130, 7, 55, 0.5, 1.0, \
                 *ATTACK_LR_CHECK_F, 0.0, true, Hash40::new(\"collision_attr_normal\"), \
                 *ATTACK_SOUND_LEVEL_L, *COLLISION_SOUND_ATTR_NONE, *ATTACK_REGION_NONE"
            ),
            "every value in its own slot, and the three unknowns untouched:\n{after}"
        );
        // Re-reading the file must produce exactly what the editor had.
        let reparsed = crate::acmd::parse_acmd_script(&after).to_hitboxes();
        assert_eq!(reparsed, edited);
    }

    #[test]
    fn a_status_edit_rewrites_only_that_ones_argument() {
        let pristine = hurt_of(HURTBOXES);
        assert_eq!(pristine.0.len(), 4, "two bones, set and taken back");

        // Make the knee fully invincible rather than intangible, on its opening call only.
        let mut edited = pristine.clone();
        edited.0[0].status = "HIT_STATUS_INVINCIBLE".into();
        let (after, report) = rewrite_hurtboxes(HURTBOXES, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 1, "one argument span, not one line");
        assert!(
            after.contains(
                r#"macros::HIT_NODE(agent, Hash40::new("kneer"), *HIT_STATUS_INVINCIBLE);"#
            ),
            "{after}"
        );
        // The three calls that did not change must be byte-identical, including the one that
        // shares this bone and the one that shares this status.
        for line in [
            r#"macros::HIT_NODE(agent, Hash40::new("legl"), *HIT_STATUS_XLU);"#,
            r#"macros::HIT_NODE(agent, Hash40::new("kneer"), *HIT_STATUS_NORMAL);"#,
            r#"macros::HIT_NODE(agent, Hash40::new("legl"), *HIT_STATUS_NORMAL);"#,
        ] {
            assert!(after.contains(line), "{line} was disturbed:\n{after}");
        }
    }

    #[test]
    fn a_new_hurtbox_range_is_inserted_at_its_one_based_frame() {
        let pristine = hurt_of(HURTBOXES);
        let mut edited = pristine.clone();
        edited.0.insert(
            0,
            crate::data::HurtboxState {
                target: crate::data::HurtTarget::Group(12),
                status: "HIT_STATUS_XLU".into(),
                active_start: 4,
                active_end: 7,
                site: 99,
            },
        );
        edited.0.insert(
            1,
            crate::data::HurtboxState {
                target: crate::data::HurtTarget::Group(12),
                status: "HIT_STATUS_NORMAL".into(),
                active_start: 8,
                active_end: 8,
                site: 100,
            },
        );
        let (after, report) =
            rewrite_hurtboxes(HURTBOXES, "mario/attack", &pristine, &edited).expect("rewrite");
        assert!(
            after.contains("frame(agent.lua_state_agent, 4.0);")
                && after.contains("macros::HIT_NO(agent, 12, *HIT_STATUS_XLU);")
        );
        assert!(after.contains("frame(agent.lua_state_agent, 8.0);"));
        assert_eq!(after.matches("macros::HIT_NO(agent, 12,").count(), 2);
        assert_eq!(after.matches("macros::HIT_NODE(agent").count(), 4);
        assert!(report.changed >= 2);
    }

    const DAMAGE_REACTION_SRC: &str = r#"unsafe extern "C" fn game_attackn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 3.0);
    if macros::is_excute(agent) {
        damage!(agent, *MA_MSC_DAMAGE_DAMAGE_NO_REACTION, *DAMAGE_NO_REACTION_MODE_ALWAYS, 0);
    }
    frame(agent.lua_state_agent, 8.0);
    if macros::is_excute(agent) {
        damage!(agent, *MA_MSC_DAMAGE_DAMAGE_NO_REACTION, *DAMAGE_NO_REACTION_MODE_NORMAL, 0);
    }
}
"#;

    #[test]
    fn a_damage_reaction_edit_rewrites_mode_and_value_without_touching_frames() {
        let pristine = crate::acmd::parse_acmd_script(DAMAGE_REACTION_SRC).to_hurtbox_conditions();
        assert_eq!(pristine.len(), 2);
        let mut edited = pristine.clone();
        edited[0].condition = crate::data::HurtboxCondition::DamageBasedArmor { threshold: 18.0 };
        let (after, report) =
            rewrite_hurtbox_conditions(DAMAGE_REACTION_SRC, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 2);
        assert!(after.contains(
            "damage!(agent, *MA_MSC_DAMAGE_DAMAGE_NO_REACTION, *DAMAGE_NO_REACTION_MODE_DAMAGE_POWER, 18);"
        ));
        assert!(after.contains(
            "damage!(agent, *MA_MSC_DAMAGE_DAMAGE_NO_REACTION, *DAMAGE_NO_REACTION_MODE_NORMAL, 0);"
        ));
    }

    #[test]
    fn a_new_damage_reaction_range_is_inserted_at_its_one_based_frame() {
        let pristine = crate::acmd::parse_acmd_script(DAMAGE_REACTION_SRC).to_hurtbox_conditions();
        let mut edited = pristine.clone();
        edited.insert(
            0,
            crate::data::HurtboxConditionState {
                condition: crate::data::HurtboxCondition::SuperArmor,
                active_start: 1,
                active_end: 1,
                site: 99,
            },
        );
        edited.insert(
            1,
            crate::data::HurtboxConditionState {
                condition: crate::data::HurtboxCondition::Normal,
                active_start: 2,
                active_end: 2,
                site: 100,
            },
        );
        let (after, report) =
            rewrite_hurtbox_conditions(DAMAGE_REACTION_SRC, "t", &pristine, &edited).unwrap();
        assert!(after.contains("frame(agent.lua_state_agent, 1.0);"));
        assert!(after.contains(
            "damage!(agent, *MA_MSC_DAMAGE_DAMAGE_NO_REACTION, *DAMAGE_NO_REACTION_MODE_ALWAYS, 0);"
        ));
        assert!(after.contains("frame(agent.lua_state_agent, 2.0);"));
        assert_eq!(after.matches("MA_MSC_DAMAGE_DAMAGE_NO_REACTION").count(), 4);
        assert!(report.changed >= 2);
    }

    /// A `WHOLE_HIT` between two `HIT_NODE`s, so a status edit has to pick the right slot *and*
    /// the site ordinals either side of it have to stay put.
    const HURTBOXES_WHOLE: &str = r#"unsafe extern "C" fn game_finalstart(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 1.0);
    if macros::is_excute(agent) {
        macros::HIT_NODE(agent, Hash40::new("kneer"), *HIT_STATUS_XLU);
        macros::WHOLE_HIT(agent, *HIT_STATUS_XLU);
        macros::HIT_NODE(agent, Hash40::new("legl"), *HIT_STATUS_XLU);
    }
}
"#;

    /// The status of a `WHOLE_HIT` lives one slot earlier than the other two members', and this
    /// path writes by slot index into the user's own text.
    ///
    /// Writing slot 2 unconditionally would land past the end of this call's arguments and drop
    /// the edit silently — the user changes a value, the panel shows the change, and their file
    /// keeps the old status. Both neighbours share the edited status, so a write to the wrong
    /// site shows up here rather than being masked.
    #[test]
    fn a_whole_body_status_edit_lands_in_the_call_and_not_past_its_arguments() {
        let pristine = hurt_of(HURTBOXES_WHOLE);
        assert_eq!(pristine.0.len(), 3, "two bones and the whole body");
        assert_eq!(pristine.0[1].target, crate::data::HurtTarget::Whole);

        let mut edited = pristine.clone();
        edited.0[1].status = "HIT_STATUS_INVINCIBLE".into();
        let (after, report) = rewrite_hurtboxes(HURTBOXES_WHOLE, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 1);
        assert!(
            after.contains("macros::WHOLE_HIT(agent, *HIT_STATUS_INVINCIBLE);"),
            "{after}"
        );
        for line in [
            r#"macros::HIT_NODE(agent, Hash40::new("kneer"), *HIT_STATUS_XLU);"#,
            r#"macros::HIT_NODE(agent, Hash40::new("legl"), *HIT_STATUS_XLU);"#,
        ] {
            assert!(after.contains(line), "{line} was disturbed:\n{after}");
        }
    }

    /// A modifier between two hurtbox calls, so a family that miscounted sites is visible.
    const ATTACK_MODS_SRC: &str = r#"unsafe extern "C" fn game_attacklw4(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 10.0);
    if macros::is_excute(agent) {
        macros::HIT_NODE(agent, Hash40::new("kneer"), *HIT_STATUS_XLU);
        macros::ATK_SET_SHIELD_SETOFF_MUL(agent, 0, 7);
        macros::ATK_POWER(agent, 1, 10);
        macros::HIT_NODE(agent, Hash40::new("legl"), *HIT_STATUS_XLU);
    }
}
"#;

    fn mods_of(text: &str) -> Vec<crate::data::AttackModState> {
        crate::acmd::parse_acmd_script(text).to_attack_mods()
    }

    /// A value edit rewrites that argument and nothing else, keeping the bare-integer spelling.
    ///
    /// The two calls sit next to each other with different macros, so writing to the wrong site
    /// shows up rather than being masked. `10` must not become `10.0`: these slots are
    /// `ToF32`-generic, so both compile, and churning an untouched spelling is the diff noise
    /// `to_f32_edit` exists to avoid.
    #[test]
    fn a_modifier_value_edit_rewrites_only_that_argument() {
        let pristine = mods_of(ATTACK_MODS_SRC);
        assert_eq!(pristine.len(), 2);
        let mut edited = pristine.clone();
        edited[1].value = 14.0;
        let (after, report) =
            rewrite_attack_mods(ATTACK_MODS_SRC, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 1);
        assert!(
            after.contains("macros::ATK_POWER(agent, 1, 14);"),
            "{after}"
        );
        assert!(
            after.contains("macros::ATK_SET_SHIELD_SETOFF_MUL(agent, 0, 7);"),
            "the neighbouring modifier was disturbed:\n{after}"
        );
    }

    /// The id is slot 1 and the value slot 2 — the one thing the corpus could not have proved.
    ///
    /// Every vanilla `ATK_SET_SHIELD_SETOFF_MUL` is the identical `(agent, 0, 7)`, so only
    /// `macros.rs` says which slot is which. Editing the id alone would silently rewrite the
    /// value if the two were ever transposed.
    #[test]
    fn a_modifier_id_edit_writes_the_id_slot_and_leaves_the_value() {
        let pristine = mods_of(ATTACK_MODS_SRC);
        let mut edited = pristine.clone();
        edited[0].id = 3;
        let (after, report) =
            rewrite_attack_mods(ATTACK_MODS_SRC, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(
            after.contains("macros::ATK_SET_SHIELD_SETOFF_MUL(agent, 3, 7);"),
            "{after}"
        );
    }

    /// The two families must not share a numbering space.
    ///
    /// Both `HIT_NODE`s here sit either side of two modifiers. If the modifiers consumed hurtbox
    /// sites — or the hurtbox calls consumed modifier sites — an edit to the *second* `HIT_NODE`
    /// would land on whatever the shifted ordinal pointed at. This is the failure `HURT_COMMANDS`
    /// already carries a warning about, checked across the family boundary.
    #[test]
    fn the_two_families_number_their_sites_independently() {
        let hurt_pristine = hurt_of(ATTACK_MODS_SRC);
        let mut hurt_edited = hurt_pristine.clone();
        hurt_edited.0[1].status = "HIT_STATUS_INVINCIBLE".into();
        let (after, report) =
            rewrite_hurtboxes(ATTACK_MODS_SRC, "t", &hurt_pristine, &hurt_edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(
            after.contains(
                r#"macros::HIT_NODE(agent, Hash40::new("legl"), *HIT_STATUS_INVINCIBLE);"#
            ),
            "the second HIT_NODE should have been edited:\n{after}"
        );
        // And nothing in the modifier lines moved.
        assert!(
            after.contains("macros::ATK_POWER(agent, 1, 10);"),
            "{after}"
        );
    }

    /// Changing which macro a modifier is, is structure rather than value.
    #[test]
    fn changing_a_modifier_to_the_other_macro_is_reported_not_written() {
        let pristine = mods_of(ATTACK_MODS_SRC);
        let mut edited = pristine.clone();
        edited[0].kind = crate::data::AttackModKind::Power;
        let (after, report) =
            rewrite_attack_mods(ATTACK_MODS_SRC, "t", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 0);
        assert!(
            report.skipped.iter().any(|s| s.contains("ATK_POWER")),
            "{report:?}"
        );
        assert_eq!(after, ATTACK_MODS_SRC, "a reported edit changes nothing");
    }

    /// Turning a whole-body state into a per-bone one is a different macro, so it is reported
    /// rather than written — the same rule `HIT_NODE` ↔ `HIT_NO` already follows, and the reason
    /// is sharper here: the target it would need has no slot to go in.
    #[test]
    fn retargeting_a_whole_body_state_to_a_bone_is_reported_not_written() {
        let pristine = hurt_of(HURTBOXES_WHOLE);
        let mut edited = pristine.clone();
        edited.0[1].target = crate::data::HurtTarget::Bone("kneer".into());
        let (after, report) = rewrite_hurtboxes(HURTBOXES_WHOLE, "t", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 0);
        assert!(
            report.skipped.iter().any(|s| s.contains("WHOLE_HIT")),
            "{report:?}"
        );
        assert_eq!(after, HURTBOXES_WHOLE, "a reported edit changes nothing");
    }

    #[test]
    fn a_bone_rename_and_a_priority_edit_land_in_their_own_calls() {
        let pristine = hurt_of(HURTBOXES);
        let mut edited = pristine.clone();
        edited.0[1].target = crate::data::HurtTarget::Bone("legr".into());
        edited.1[0].pri = 150;
        let (after, report) = rewrite_hurtboxes(HURTBOXES, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 2);
        assert!(
            after.contains(r#"macros::HIT_NODE(agent, Hash40::new("legr"), *HIT_STATUS_XLU);"#),
            "{after}"
        );
        assert!(after.contains("macros::COL_PRI(agent, 150);"), "{after}");
        // `COL_PRI` is the third hurtbox call in the file but the first priority span. Pairing
        // those two lists by position rather than by site would have written 150 into a
        // `HIT_NODE`.
        assert!(
            after.contains(r#"macros::HIT_NODE(agent, Hash40::new("kneer"), *HIT_STATUS_XLU);"#),
            "the priority edit must not have landed in a bone call:\n{after}"
        );
    }

    /// `HIT_NODE` and `HIT_NO` take a hash and an integer in the same slot. Retuning across
    /// that boundary would write a `Hash40::new(…)` where the macro wants a number — a call
    /// that does not compile — so it is reported instead.
    #[test]
    fn changing_a_bone_into_a_numbered_group_is_reported_rather_than_written() {
        let pristine = hurt_of(HURTBOXES);
        let mut edited = pristine.clone();
        edited.0[0].target = crate::data::HurtTarget::Group(8);
        let (after, report) = rewrite_hurtboxes(HURTBOXES, "t", &pristine, &edited).unwrap();
        assert_eq!(after, HURTBOXES, "nothing may be written");
        assert!(
            report
                .skipped
                .iter()
                .any(|s| s.contains("HIT_NODE") && s.contains("HIT_NO")),
            "the report must name both macros: {report:?}"
        );
    }

    /// A state's frame is the `frame(...)` block it sits in, not an argument of the call, so
    /// moving one has to be reported the way a retimed hitbox is.
    #[test]
    fn retiming_a_hurtbox_state_is_reported_and_its_other_edits_still_land() {
        let pristine = hurt_of(HURTBOXES);
        let mut edited = pristine.clone();
        edited.0[0].active_start = 4;
        edited.0[0].status = "HIT_STATUS_OFF".into();
        let (after, report) = rewrite_hurtboxes(HURTBOXES, "t", &pristine, &edited).unwrap();
        assert!(
            report.skipped.iter().any(|s| s.contains("retimed")),
            "{report:?}"
        );
        assert!(
            after.contains(r#"macros::HIT_NODE(agent, Hash40::new("kneer"), *HIT_STATUS_OFF);"#),
            "the value edit is still worth writing:\n{after}"
        );
    }

    /// `kirby/TurnDash` verbatim — the sound write-back's fixture.
    ///
    /// Chosen because it holds all three shapes in one function: a one-hash call, the one
    /// member with a trailing non-hash argument, and two `PLAY_STEP_FLIPPABLE`s that name the
    /// same two sounds in opposite order. That last pair is what a write-back keyed on the
    /// *sound name* rather than on the site would get wrong.
    const SOUNDS: &str = r#"unsafe extern "C" fn sound_turndash(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        macros::PLAY_SE(agent, Hash40::new("se_kirby_dash_start"));
        macros::SET_PLAY_INHIVIT(agent, Hash40::new("se_kirby_dash_start"), 20);
    }
    wait(agent.lua_state_agent, 13.0);
    if macros::is_excute(agent) {
        macros::PLAY_STEP_FLIPPABLE(agent, Hash40::new("se_kirby_step_left_m"), Hash40::new("se_kirby_step_right_m"));
    }
    wait(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::PLAY_STEP_FLIPPABLE(agent, Hash40::new("se_kirby_step_right_m"), Hash40::new("se_kirby_step_left_m"));
    }
}
"#;

    fn sounds_of(text: &str) -> Vec<crate::data::SoundEvent> {
        crate::acmd::parse_sound_script(text).to_sound_events()
    }

    /// Renaming a sound rewrites that one argument and leaves the rest of the file alone.
    ///
    /// The second footstep is the interesting one: it names the same two sounds as the first,
    /// swapped. Editing only its left channel has to leave three other spans holding those very
    /// strings untouched, which is what says the edit was placed by site.
    #[test]
    fn renaming_one_sound_rewrites_only_that_argument() {
        let pristine = sounds_of(SOUNDS);
        let mut edited = pristine.clone();
        edited[3].call.sounds[0] = "se_common_step_left_m".into();
        let (after, report) = rewrite_sounds(SOUNDS, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 1);
        assert!(
            after.contains(
                r#"macros::PLAY_STEP_FLIPPABLE(agent, Hash40::new("se_common_step_left_m"), Hash40::new("se_kirby_step_left_m"));"#
            ),
            "{after}"
        );
        // The first footstep names the same two sounds the other way round and must be intact.
        assert!(
            after.contains(
                r#"macros::PLAY_STEP_FLIPPABLE(agent, Hash40::new("se_kirby_step_left_m"), Hash40::new("se_kirby_step_right_m"));"#
            ),
            "the edit landed in the wrong footstep:\n{after}"
        );
        // And exactly one line differs — an argument rewrite, not a re-emission of the function.
        let changed_lines = SOUNDS
            .lines()
            .zip(after.lines())
            .filter(|(before, now)| before != now)
            .count();
        assert_eq!(changed_lines, 1, "the write-back reformatted the file");
    }

    /// A sound in one macro moved to another is structure, not a value, and is reported.
    ///
    /// `PLAY_SE` and `PLAY_STEP_FLIPPABLE` do not even take the same number of arguments, so
    /// writing one over the other produces a call that does not compile. The report names the
    /// macro because that is the part the user has to undo.
    #[test]
    fn changing_which_sound_macro_is_called_is_reported_rather_than_written() {
        let pristine = sounds_of(SOUNDS);
        let mut edited = pristine.clone();
        edited[0].call.func = "PLAY_STEP_FLIPPABLE".into();
        let (after, report) = rewrite_sounds(SOUNDS, "t", &pristine, &edited).unwrap();
        assert_eq!(after, SOUNDS, "nothing may be written");
        assert!(
            report
                .skipped
                .iter()
                .any(|s| s.contains("PLAY_SE") && s.contains("PLAY_STEP_FLIPPABLE")),
            "the report must name both macros: {report:?}"
        );
    }

    /// Moving a sound to another frame is reported, and a rename in the same pass still lands.
    #[test]
    fn retiming_a_sound_is_reported_and_other_edits_still_land() {
        let pristine = sounds_of(SOUNDS);
        let mut edited = pristine.clone();
        edited[0].frame = 9;
        edited[0].call.sounds[0] = "se_common_dash_start".into();
        edited[1].call.sounds[0] = "se_common_dash_start".into();
        let (after, report) = rewrite_sounds(SOUNDS, "t", &pristine, &edited).unwrap();
        assert!(
            report.skipped.iter().any(|s| s.contains("retimed")),
            "{report:?}"
        );
        assert!(
            !after.contains(r#"macros::PLAY_SE(agent, Hash40::new("se_common_dash_start"));"#),
            "a retimed call must not also be rewritten:\n{after}"
        );
        assert!(
            after.contains(
                r#"macros::SET_PLAY_INHIVIT(agent, Hash40::new("se_common_dash_start"), 20);"#
            ),
            "the untouched call's own rename is still worth writing:\n{after}"
        );
    }

    /// A sound call written with the wrong number of arguments is not a site.
    ///
    /// The parser refuses one and leaves the line `Raw`, so the IR never numbers it. A scan
    /// matching on the macro name alone would number it anyway, and every site after it would
    /// resolve one line too early — here, renaming the footstep would rewrite the broken call
    /// above it and leave the footstep untouched.
    ///
    /// Nothing in the 301-script corpus is written this way, so the corpus oracle cannot reach
    /// it, and a mutation removing the arity filter passed every other test in this file. Hand
    /// authoring is exactly where it would come from: it is a mistake a person makes and the
    /// game's own scripts do not.
    #[test]
    fn a_malformed_sound_call_does_not_take_a_site() {
        const MALFORMED: &str = r#"unsafe extern "C" fn sound_typo(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        macros::PLAY_SE(agent);
        macros::PLAY_SE(agent, Hash40::new("se_kirby_dash_start"));
    }
}
"#;
        assert_eq!(
            sound_sites(MALFORMED).len(),
            1,
            "the argument-less call must not be scanned as a site"
        );

        let pristine = sounds_of(MALFORMED);
        assert_eq!(pristine.len(), 1, "only the well-formed call is an event");
        let mut edited = pristine.clone();
        edited[0].call.sounds[0] = "se_common_dash_start".into();
        let (after, report) = rewrite_sounds(MALFORMED, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(
            after.contains(r#"macros::PLAY_SE(agent, Hash40::new("se_common_dash_start"));"#),
            "the rename did not land on the call it named:\n{after}"
        );
        assert!(
            after.contains("macros::PLAY_SE(agent);"),
            "the broken call was rewritten instead:\n{after}"
        );
    }

    /// A looped call is one line and several events, so the same span is reached more than once.
    ///
    /// Writing it twice would hand `apply` two edits over the same bytes. Agreeing edits collapse
    /// to one; disagreeing ones are a genuine conflict the user has to resolve, and are reported
    /// rather than decided by whichever iteration happened to be last in the list.
    #[test]
    fn a_looped_sound_edited_once_is_written_once() {
        const LOOPED: &str = r#"unsafe extern "C" fn sound_looped(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 6.0);
    for _ in 0..3 {
    wait(agent.lua_state_agent, 2.0);
    if macros::is_excute(agent) {
        macros::PLAY_SE(agent, Hash40::new("se_kirby_dash_start"));
    }
    }
}
"#;
        let pristine = sounds_of(LOOPED);
        assert_eq!(pristine.len(), 3, "the loop should unroll to three events");

        let mut edited = pristine.clone();
        for event in &mut edited {
            event.call.sounds[0] = "se_common_dash_start".into();
        }
        let (after, report) = rewrite_sounds(LOOPED, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 1, "one line, one edit");
        assert_eq!(
            after.matches("se_common_dash_start").count(),
            1,
            "the loop body was written three times:\n{after}"
        );

        // Two iterations disagreeing is not something the panel can produce today — it edits the
        // statement, so every event moves together. It is asserted anyway because the *next*
        // thing to touch this, a per-event editor, would produce it silently.
        let mut split = pristine.clone();
        split[0].call.sounds[0] = "se_common_dash_start".into();
        split[1].call.sounds[0] = "se_common_dash_stop".into();
        let (_, report) = rewrite_sounds(LOOPED, "t", &pristine, &split).unwrap();
        assert!(
            report.skipped.iter().any(|s| s.contains("two different")),
            "a conflict must be reported, not resolved: {report:?}"
        );
    }

    /// The write-back matches a call to its line by counting spawn macros in source order, so
    /// C6 teaching the parser to nest conditionals instead of flattening them could have shifted
    /// every ordinal in a guarded script. It does not — the ordinal walk descends into a `Cond`
    /// body in place — and this pins that on the shape that would break first.
    ///
    /// The costume tint matters for a second reason. It is carried through the *export*
    /// verbatim, but the write-back edits the user's own file, which already contains it. So it
    /// has to come out of this untouched: a sync that also wrote the carried copy would be
    /// stacking the export's duplicate on top of the original.
    #[test]
    fn a_costume_guard_does_not_shift_the_write_backs_ordinals() {
        const GUARDED: &str = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 9.0);
    if get_value_float(agent.lua_state_agent, *SO_VAR_FLOAT_LR) < 0.0 {
        if macros::is_excute(agent) {
            macros::EFFECT_FOLLOW(agent, Hash40::new("dolly_roll_l"), Hash40::new("throw"), 0, 2.5, 0, 0, 0, 0, 1, true);
        }
    }
    if(0x2508e0(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 0)){
        if macros::is_excute(agent) {
            macros::LAST_EFFECT_SET_COLOR(agent, 0.146, 0.205, 0.333);
        }
    }
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("dolly_roll_r"), Hash40::new("throw"), 0, 2.5, 0, 0, 0, 0, 1, true);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(GUARDED).to_effect_calls();
        assert_eq!(pristine.len(), 2, "both spawns are still resolved");
        assert_eq!(
            pristine[0].guard.as_deref(),
            Some("if get_value_float(agent.lua_state_agent, *SO_VAR_FLOAT_LR) < 0.0 {"),
            "the first spawn is the guarded one"
        );

        // Edit the SECOND spawn — the one past both the guard and the carried tint. Had the
        // conditional shifted the ordinals, this would land on the first spawn's line instead.
        let mut edited = pristine.clone();
        edited[1].offset = [1.5, 2.5, 0.0];
        let (after, report) = rewrite_effect_calls(GUARDED, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(
            after.contains(r#"Hash40::new("dolly_roll_r"), Hash40::new("throw"), 1.5, 2.5, 0"#),
            "the edit must land on the spawn past the guard:\n{after}"
        );
        assert!(
            after.contains(r#"Hash40::new("dolly_roll_l"), Hash40::new("throw"), 0, 2.5, 0"#),
            "the guarded spawn must be untouched:\n{after}"
        );
        assert_eq!(
            after.matches("LAST_EFFECT_SET_COLOR").count(),
            1,
            "the tint is already in the user's file; carrying it through the export must not \
             also insert a second copy here:\n{after}"
        );
        assert!(
            after.contains("macros::LAST_EFFECT_SET_COLOR(agent, 0.146, 0.205, 0.333);"),
            "and its value must not move:\n{after}"
        );
    }

    #[test]
    fn a_tint_edit_rewrites_only_its_own_spawns_colour_line() {
        let pristine = crate::acmd::parse_effect_script(MODIFIERS).to_effect_calls();
        assert_eq!(pristine.len(), 2);
        // The rate below the tint still found its spawn, which is the whole point of the
        // fixture: a modifier does not break the run for the modifier after it.
        assert_eq!(pristine[0].tint, Some([0.25, 1.3, 2.5]));
        assert_eq!(pristine[0].rate, Some(2.0));
        assert_eq!(pristine[1].alpha, Some(0.7));

        let mut edited = pristine.clone();
        edited[0].tint = Some([0.25, 0.5, 2.5]);
        let (after, report) = rewrite_effect_calls(MODIFIERS, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        // One component moved, so one argument span is rewritten — not the whole line, and not
        // the two components that did not change.
        assert_eq!(report.changed, 1);
        assert!(
            after.contains("macros::LAST_EFFECT_SET_COLOR(agent, 0.25, 0.5, 2.5);"),
            "only the green component may move:\n{after}"
        );
        assert!(
            after.contains("macros::LAST_EFFECT_SET_RATE(agent, 2);")
                && after.contains("macros::LAST_EFFECT_SET_ALPHA(agent, 0.7);"),
            "recolouring one spawn must not disturb any other modifier:\n{after}"
        );
    }

    #[test]
    fn an_opacity_edit_rewrites_only_its_own_spawns_alpha_line() {
        let pristine = crate::acmd::parse_effect_script(MODIFIERS).to_effect_calls();
        let mut edited = pristine.clone();
        edited[1].alpha = Some(0.3);
        let (after, report) = rewrite_effect_calls(MODIFIERS, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 1);
        assert!(
            after.contains("macros::LAST_EFFECT_SET_ALPHA(agent, 0.3);"),
            "{after}"
        );
        assert!(
            after.contains("macros::LAST_EFFECT_SET_COLOR(agent, 0.25, 1.3, 2.5);"),
            "the other spawn's tint must be untouched:\n{after}"
        );
    }

    /// Each modifier lives on a line of its own, so switching one on or off adds or deletes a
    /// call. Structural, and named rather than guessed at — the same rule the rate follows, and
    /// the reason all three share `modifier_edits`.
    #[test]
    fn turning_a_tint_or_opacity_on_or_off_is_reported_rather_than_written() {
        let pristine = crate::acmd::parse_effect_script(MODIFIERS).to_effect_calls();

        let mut removed = pristine.clone();
        removed[0].tint = None;
        let (after, report) = rewrite_effect_calls(MODIFIERS, "t", &pristine, &removed).unwrap();
        assert_eq!(after, MODIFIERS, "the user's line must still be there");
        assert!(
            report
                .skipped
                .iter()
                .any(|s| s.contains("lost its tint") && s.contains("LAST_EFFECT_SET_COLOR")),
            "{report:?}"
        );

        // The second spawn has no colour line at all, so there is nowhere for one to be written.
        let mut added = pristine.clone();
        added[1].tint = Some([1.0, 0.0, 0.0]);
        let (after, report) = rewrite_effect_calls(MODIFIERS, "t", &pristine, &added).unwrap();
        assert_eq!(after, MODIFIERS, "nothing may be inserted into the script");
        assert!(
            report.skipped.iter().any(|s| s.contains("gained a tint")),
            "{report:?}"
        );

        let mut faded = pristine.clone();
        faded[1].alpha = None;
        let (after, report) = rewrite_effect_calls(MODIFIERS, "t", &pristine, &faded).unwrap();
        assert_eq!(after, MODIFIERS);
        assert!(
            report
                .skipped
                .iter()
                .any(|s| s.contains("lost its opacity") && s.contains("LAST_EFFECT_SET_ALPHA")),
            "{report:?}"
        );
    }

    /// Kirby's dash attack, verbatim: a spawn and four colour commands, which is what makes
    /// this a test of the ordinals as much as of the values. A colour command produces a call,
    /// so it consumes an ordinal; if it did not, every edit after the first `BURN_COLOR` would
    /// be written into the line belonging to a different call.
    const COLORS: &str = r#"unsafe extern "C" fn effect_attackdash(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_NO_STOP(agent, Hash40::new("kirby_dash"), Hash40::new("top"), 0, 6, 5, -90, 0, 160, 0.7, true);
    }
    frame(agent.lua_state_agent, 9.0);
    if macros::is_excute(agent) {
        macros::BURN_COLOR(agent, 2, 0.059, 0.008, 0);
        macros::BURN_COLOR_FRAME(agent, 4, 2, 0.059, 0.008, 0.9);
    }
    frame(agent.lua_state_agent, 42.0);
    if macros::is_excute(agent) {
        macros::BURN_COLOR_NORMAL(agent);
    }
}
"#;

    #[test]
    fn a_colour_edit_rewrites_only_its_own_commands_arguments() {
        let pristine = crate::acmd::parse_effect_script(COLORS).to_effect_calls();
        assert_eq!(pristine.len(), 4);

        let mut edited = pristine.clone();
        // The interpolating half of the pair only: the snap above it keeps its own values, so
        // a wrong ordinal or a wrong slot table shows up as the wrong line changing.
        edited[2].color = Some(crate::data::ColorCall {
            transition: Some(6.0),
            rgba: Some([2.0, 0.5, 0.25, 0.9]),
        });

        let (after, report) = rewrite_effect_calls(COLORS, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(
            after.contains("macros::BURN_COLOR(agent, 2, 0.059, 0.008, 0);"),
            "the snap above must be untouched:\n{after}"
        );
        assert!(
            after.contains("macros::BURN_COLOR_FRAME(agent, 6, 2, 0.5, 0.25, 0.9);"),
            "the length is slot 1 and the colour follows it — and 6, not 6.0:\n{after}"
        );
        assert!(
            after.contains("macros::BURN_COLOR_NORMAL(agent);"),
            "an argument-less command has nothing to write and must survive:\n{after}"
        );
        // Everything else in the file, comments and formatting included, byte for byte.
        assert_eq!(
            after.replace("macros::BURN_COLOR_FRAME(agent, 6, 2, 0.5, 0.25, 0.9);", ""),
            COLORS.replace(
                "macros::BURN_COLOR_FRAME(agent, 4, 2, 0.059, 0.008, 0.9);",
                ""
            ),
        );
    }

    /// Swapping `BURN_COLOR` for `BURN_COLOR_FRAME` adds an argument the existing call does not
    /// have. That is a change of command, not of value, so it lands in an export and is
    /// reported here — writing the length into slot 1 would put it in the red channel.
    #[test]
    fn changing_which_colour_command_a_call_is_gets_reported() {
        let pristine = crate::acmd::parse_effect_script(COLORS).to_effect_calls();
        let mut edited = pristine.clone();
        edited[1].spawn_func = "BURN_COLOR_FRAME".into();
        edited[1].color = Some(crate::data::ColorCall {
            transition: Some(3.0),
            rgba: Some([2.0, 0.059, 0.008, 0.0]),
        });

        let (after, report) = rewrite_effect_calls(COLORS, "t", &pristine, &edited).unwrap();
        assert_eq!(
            after, COLORS,
            "the user's call must be left exactly as written"
        );
        assert!(
            report.skipped.iter().any(|s| {
                s.contains("changed graphic, joint, spawn command, timing, or enablement")
            }),
            "{report:?}"
        );
    }

    /// Selecting calls to sync by transform difference alone meant an edit that changed only
    /// the graphic was neither written nor reported: Save looked like it worked and the
    /// source still said the old name.
    #[test]
    fn an_edit_that_changes_no_transform_value_is_still_reported() {
        let text = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_attack"), Hash40::new("top"), 0, 1, 0, 0, 0, 0, 1, true);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(text).to_effect_calls();
        assert_eq!(pristine.len(), 1);

        for mutate in [
            (|c: &mut crate::data::EffectCall| c.effect_name = "sys_renamed".into())
                as fn(&mut crate::data::EffectCall),
            |c| c.bone_name = "havel".into(),
            |c| c.disabled = true,
        ] {
            let mut edited = pristine.clone();
            mutate(&mut edited[0]);
            let (after, report) = rewrite_effect_calls(text, "t", &pristine, &edited).unwrap();
            assert_eq!(after, text, "nothing structural may be guessed at");
            assert_eq!(
                report.skipped.len(),
                1,
                "an unwritable edit must be named, not dropped: {report:?}"
            );
        }
    }

    #[test]
    fn a_flat_one_shot_retime_moves_its_frame_block_and_canonicalizes_end() {
        let source = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_attack"), Hash40::new("top"), 0, 1, 0, 0, 0, 0, 1, false);
    }
    frame(agent.lua_state_agent, 12.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_late"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, false);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(source).to_effect_calls();
        assert_eq!(pristine.len(), 2);
        let mut edited = pristine.clone();
        edited[0].active_start = 8;
        edited[0].active_end = 9999;

        let (after, report) = rewrite_effect_calls(source, "t", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(
            after.contains("frame(agent.lua_state_agent, 8.0);"),
            "{after}"
        );
        assert!(
            !after.contains("frame(agent.lua_state_agent, 5.0);"),
            "{after}"
        );
        let reparsed = crate::acmd::parse_effect_script(&after).to_effect_calls();
        assert_eq!((reparsed[0].active_start, reparsed[0].active_end), (8, 8));
    }

    #[test]
    fn a_zero_frame_retime_is_canonicalized_and_can_be_retimed_again() {
        let source = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_attack"), Hash40::new("top"), 0, 1, 0, 0, 0, 0, 1, false);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(source).to_effect_calls();
        let mut zero = pristine.clone();
        zero[0].active_start = 0;
        zero[0].active_end = 9999;

        let (at_first, first_report) = rewrite_effect_calls(source, "zero", &pristine, &zero)
            .expect("zero-frame retime should be handled as the first script frame");
        assert!(first_report.skipped.is_empty(), "{first_report:?}");
        assert_eq!(first_report.changed, 1, "{first_report:?}");
        assert!(
            at_first.contains("frame(agent.lua_state_agent, 1.0);"),
            "{at_first}"
        );
        let first_calls = crate::acmd::parse_effect_script(&at_first).to_effect_calls();
        assert_eq!(
            (first_calls[0].active_start, first_calls[0].active_end),
            (1, 1)
        );

        let mut later = first_calls.clone();
        later[0].active_start = 8;
        later[0].active_end = 9999;
        let (at_later, later_report) =
            rewrite_effect_calls(&at_first, "zero-then-later", &first_calls, &later)
                .expect("a later retime must remain available after frame zero was entered");
        assert!(later_report.skipped.is_empty(), "{later_report:?}");
        assert_eq!(later_report.changed, 1, "{later_report:?}");
        assert!(
            at_later.contains("frame(agent.lua_state_agent, 8.0);"),
            "{at_later}"
        );
        let reparsed = crate::acmd::parse_effect_script(&at_later).to_effect_calls();
        assert_eq!((reparsed[0].active_start, reparsed[0].active_end), (8, 8));
    }

    #[test]
    fn a_retime_and_value_edit_move_one_block_atomically() {
        let source = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_attack"), Hash40::new("top"), 0, 1, 0, 0, 0, 0, 1, true);
        macros::LAST_EFFECT_SET_RATE(agent, 1);
    }
    frame(agent.lua_state_agent, 20.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_late"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, false);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(source).to_effect_calls();
        let mut edited = pristine.clone();
        edited[0].active_start = 8;
        edited[0].offset[0] = 2.5;
        edited[0].rate = Some(2.0);

        let (after, report) = rewrite_effect_calls(source, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 3, "{report:?}");
        assert!(
            after.contains("frame(agent.lua_state_agent, 8.0);"),
            "{after}"
        );
        assert!(after.contains("Hash40::new(\"top\"), 2.5, 1, 0"), "{after}");
        assert!(
            after.contains("macros::LAST_EFFECT_SET_RATE(agent, 2);"),
            "{after}"
        );
        assert!(
            !after.contains("frame(agent.lua_state_agent, 5.0);"),
            "{after}"
        );
        let reparsed = crate::acmd::parse_effect_script(&after).to_effect_calls();
        assert_eq!(reparsed[0].active_start, 8);
        assert_eq!(reparsed[0].active_end, 9999);
        assert_eq!(reparsed[0].rate, Some(2.0));
    }

    #[test]
    fn a_follow_retime_moves_its_unique_finite_stop_with_the_start() {
        let source = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_attack"), Hash40::new("top"), 0, 1, 0, 0, 0, 0, 1, true);
    }
    frame(agent.lua_state_agent, 12.0);
    if macros::is_excute(agent) {
        macros::EFFECT_OFF_KIND(agent, Hash40::new("sys_attack"), false, true);
    }
    frame(agent.lua_state_agent, 20.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_late"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, false);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(source).to_effect_calls();
        assert_eq!(pristine[0].active_end, 12);
        let mut edited = pristine.clone();
        edited[0].active_start = 8;
        edited[0].active_end = 15;

        let (after, report) = rewrite_effect_calls(source, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 2, "{report:?}");
        assert!(
            after.contains("frame(agent.lua_state_agent, 8.0);"),
            "{after}"
        );
        assert!(
            after.contains("frame(agent.lua_state_agent, 15.0);"),
            "{after}"
        );
        assert!(
            !after.contains("frame(agent.lua_state_agent, 5.0);"),
            "{after}"
        );
        assert!(
            !after.contains("frame(agent.lua_state_agent, 12.0);"),
            "{after}"
        );
        let reparsed = crate::acmd::parse_effect_script(&after).to_effect_calls();
        assert_eq!((reparsed[0].active_start, reparsed[0].active_end), (8, 15));
    }

    #[test]
    fn colour_and_control_point_events_use_the_same_safe_retime_path() {
        let source = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::FLASH(agent, 1, 0.2, 0.3, 0.4);
    }
    frame(agent.lua_state_agent, 12.0);
    if macros::is_excute(agent) {
        macros::ENABLE_AREA(agent, 3);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(source).to_effect_calls();
        assert_eq!(pristine.len(), 2);
        let mut edited = pristine.clone();
        edited[0].active_start = 8;
        edited[0].active_end = 9999;
        edited[1].active_start = 16;
        edited[1].active_end = 9999;

        let (after, report) = rewrite_effect_calls(source, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 2, "{report:?}");
        assert!(
            after.contains("frame(agent.lua_state_agent, 8.0);"),
            "{after}"
        );
        assert!(
            after.contains("frame(agent.lua_state_agent, 16.0);"),
            "{after}"
        );
        let reparsed = crate::acmd::parse_effect_script(&after).to_effect_calls();
        assert_eq!((reparsed[0].active_start, reparsed[0].active_end), (8, 8));
        assert_eq!((reparsed[1].active_start, reparsed[1].active_end), (16, 16));
    }

    #[test]
    fn a_named_trail_retime_moves_its_paired_after_image_off_block() {
        let source = format!(
            "{}\n    frame(agent.lua_state_agent, 20.0);\n    if macros::is_excute(agent) {{\n        macros::AFTER_IMAGE_OFF(agent, 3);\n    }}\n}}\n",
            TRAIL.strip_suffix("}\n").unwrap()
        );
        let pristine = crate::acmd::parse_effect_script(&source).to_effect_calls();
        assert_eq!(pristine.len(), 1, "{pristine:#?}");
        assert_eq!(
            pristine[0].trail_command.as_deref(),
            Some("AFTER_IMAGE4_ON_arg29")
        );
        assert_eq!(pristine[0].active_end, 20);
        let mut edited = pristine.clone();
        edited[0].active_start = 7;
        edited[0].active_end = 18;

        let (after, report) = rewrite_effect_calls(&source, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 2, "{report:?}");
        assert!(
            after.contains("frame(agent.lua_state_agent, 7.0);"),
            "{after}"
        );
        assert!(
            after.contains("frame(agent.lua_state_agent, 18.0);"),
            "{after}"
        );
        assert!(
            !after.contains("frame(agent.lua_state_agent, 5.0);"),
            "{after}"
        );
        assert!(
            !after.contains("frame(agent.lua_state_agent, 20.0);"),
            "{after}"
        );
        let reparsed = crate::acmd::parse_effect_script(&after).to_effect_calls();
        assert_eq!((reparsed[0].active_start, reparsed[0].active_end), (7, 18));
    }

    #[test]
    fn effect_timing_moves_refuse_branches_and_loops_but_move_a_spawn_on_its_own() {
        let branch = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if get_value_float(agent.lua_state_agent, *SO_VAR_FLOAT_LR) < 0.0 {
        if macros::is_excute(agent) {
            macros::EFFECT(agent, Hash40::new("sys_attack"), Hash40::new("top"), 0, 1, 0, 0, 0, 0, 1, false);
        }
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(branch).to_effect_calls();
        assert_eq!(pristine.len(), 1);
        let mut edited = pristine.clone();
        edited[0].active_start = 8;
        let (after, report) = rewrite_effect_calls(branch, "branch", &pristine, &edited).unwrap();
        assert_eq!(after, branch);
        assert!(
            report
                .skipped
                .iter()
                .any(|note| note.contains("branch, loop, wait")),
            "{report:?}"
        );

        let looped = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    for _ in 0..2 {
        if macros::is_excute(agent) {
            macros::EFFECT(agent, Hash40::new("sys_attack"), Hash40::new("top"), 0, 1, 0, 0, 0, 0, 1, false);
        }
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(looped).to_effect_calls();
        assert_eq!(pristine.len(), 2);
        let mut edited = pristine.clone();
        edited[0].active_start = 8;
        let (after, report) = rewrite_effect_calls(looped, "loop", &pristine, &edited).unwrap();
        assert_eq!(after, looped);
        assert!(
            report.skipped.iter().any(|note| note.contains("loop")),
            "{report:?}"
        );

        let collision = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_attack"), Hash40::new("top"), 0, 1, 0, 0, 0, 0, 1, false);
    }
    frame(agent.lua_state_agent, 10.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_late"), Hash40::new("top"), 0, 0, 0, 0, 0, 0, 1, false);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(collision).to_effect_calls();
        let mut edited = pristine.clone();
        edited[0].active_start = 10;
        // A frame that already has a block is no longer a refusal: the line mover appends the
        // spawn to it, after what that frame already spawns.
        let (after, report) =
            rewrite_effect_calls(collision, "collision", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        let moved = crate::acmd::parse_effect_script(&after).to_effect_calls();
        assert!(moved.iter().all(|call| call.active_start == 10), "{after}");
        assert!(after.find("sys_late") < after.find("sys_attack"), "{after}");

        let unrelated = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new("sys_attack"), Hash40::new("top"), 0, 1, 0, 0, 0, 0, 1, false);
        macros::COL_PRI(agent, 2);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(unrelated).to_effect_calls();
        assert_eq!(pristine.len(), 1);
        let mut edited = pristine.clone();
        edited[0].active_start = 8;
        // The spawn moves on its own; the unrelated line keeps the frame it runs on.
        let (after, report) =
            rewrite_effect_calls(unrelated, "unrelated", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        let frame_eight = after.find("frame(agent.lua_state_agent, 8.0);").expect("a frame-8 block");
        assert!(after.find("COL_PRI").unwrap() < frame_eight, "{after}");
        assert!(after.find("sys_attack").unwrap() > frame_eight, "{after}");
    }

    #[test]
    fn follow_timing_moves_refuse_stop_creation_removal_and_ambiguous_pairing() {
        let no_stop = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_attack"), Hash40::new("top"), 0, 1, 0, 0, 0, 0, 1, true);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(no_stop).to_effect_calls();
        let mut edited = pristine.clone();
        edited[0].active_end = 12;
        let (after, report) =
            rewrite_effect_calls(no_stop, "create-stop", &pristine, &edited).unwrap();
        assert_eq!(after, no_stop);
        assert!(
            report
                .skipped
                .iter()
                .any(|note| note.contains("creating or removing a stop")),
            "{report:?}"
        );

        let finite = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_attack"), Hash40::new("top"), 0, 1, 0, 0, 0, 0, 1, true);
    }
    frame(agent.lua_state_agent, 12.0);
    if macros::is_excute(agent) {
        macros::EFFECT_OFF_KIND(agent, Hash40::new("sys_attack"), false, true);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(finite).to_effect_calls();
        let mut edited = pristine.clone();
        edited[0].active_end = 9999;
        let (after, report) =
            rewrite_effect_calls(finite, "remove-stop", &pristine, &edited).unwrap();
        assert_eq!(after, finite);
        assert!(
            report
                .skipped
                .iter()
                .any(|note| note.contains("creating or removing a stop")),
            "{report:?}"
        );

        let ambiguous = r#"unsafe extern "C" fn effect_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_attack"), Hash40::new("top"), 0, 1, 0, 0, 0, 0, 1, true);
    }
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new("sys_attack"), Hash40::new("top"), 0, 2, 0, 0, 0, 0, 1, true);
    }
    frame(agent.lua_state_agent, 12.0);
    if macros::is_excute(agent) {
        macros::EFFECT_OFF_KIND(agent, Hash40::new("sys_attack"), false, true);
    }
    frame(agent.lua_state_agent, 12.0);
    if macros::is_excute(agent) {
        macros::EFFECT_OFF_KIND(agent, Hash40::new("sys_attack"), false, true);
    }
}
"#;
        let pristine = crate::acmd::parse_effect_script(ambiguous).to_effect_calls();
        assert_eq!(pristine.len(), 2);
        assert!(pristine.iter().all(|call| call.active_end == 12));
        let mut edited = pristine.clone();
        edited[0].active_end = 15;
        let (after, report) =
            rewrite_effect_calls(ambiguous, "ambiguous", &pristine, &edited).unwrap();
        assert_eq!(after, ambiguous);
        assert!(
            report
                .skipped
                .iter()
                .any(|note| note.contains("unique paired stop")),
            "{report:?}"
        );
    }

    /// The write-back covered eleven numeric slots; every other property the hitbox panels
    /// expose — the joint, the masks, the sound and collision attributes, the flags — was
    /// silently discarded on Save.
    #[test]
    fn every_hitbox_property_the_panels_expose_reaches_the_source() {
        let text = r#"unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("top"), 10.0, 361, 100, 0, 30, 4.0, 0.0, 8.0, 0.0, None, None, None, 1.0, 1.0, *ATTACK_SETOFF_KIND_OFF, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, true, true, false, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_M, *COLLISION_SOUND_ATTR_PUNCH, *ATTACK_REGION_PUNCH);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_hitboxes();
        assert_eq!(pristine.len(), 1, "{pristine:#?}");

        let mut edited = pristine.clone();
        let h = &mut edited[0];
        h.bone_name = "ArmL".into();
        h.capsule_end = Some([1.0, 2.0, 3.0]);
        h.setoff_kind = "ATTACK_SETOFF_KIND_ON".into();
        h.lr_check = "ATTACK_LR_CHECK_POS".into();
        h.is_clang = true;
        h.is_add_attack = 1;
        h.hitbox_attr = 2.0;
        h.ground_or_air = 1;
        h.is_mtk = true;
        h.is_shield_disable = true;
        h.is_reflectable = false;
        h.is_absorbable = false;
        h.is_landing_attack = true;
        h.situation_mask = "COLLISION_SITUATION_MASK_G".into();
        h.category_mask = "COLLISION_CATEGORY_MASK_CAT1".into();
        h.part_mask = "COLLISION_PART_MASK_HEAD".into();
        h.no_finish_camera = true;
        h.collision_attr = "collision_attr_fire".into();
        h.sound_level = "ATTACK_SOUND_LEVEL_L".into();
        h.sound_attr = "COLLISION_SOUND_ATTR_KICK".into();
        h.attack_region = "ATTACK_REGION_KICK".into();

        let (after, report) = rewrite_hitboxes(text, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");

        // The written source must parse back to exactly what the editor holds — the real
        // test, since it covers spelling as well as slot numbering. The one intended
        // difference is the joint: skeletons expose `ArmL`, ACMD hashes `arml`, and both
        // `emit_attack` and live injection write the lowercase name.
        let round_tripped = crate::acmd::parse_acmd_script(&after).to_hitboxes();
        edited[0].bone_name = "arml".into();
        assert_eq!(round_tripped, edited, "\n{after}");
        assert!(after.contains(r#"Hash40::new("arml")"#), "{after}");
        assert!(after.contains("Some(1.0), Some(2.0), Some(3.0)"), "{after}");
    }

    /// The flip side: syncing a hitbox nobody edited must not restyle the call. The editor
    /// holds these values decoded, so writing every slot back would respell the whole file.
    #[test]
    fn an_unedited_hitbox_is_left_byte_identical() {
        let text = r#"unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("top"), 10.0, 361, 100, 0, 30, 4.0, 0.0, 8.0, 0.0, None, None, None, 1.0, 1.0, 0, 1, false, 0, 0.0, 0, false, false, true, true, false, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_M, *COLLISION_SOUND_ATTR_PUNCH, *ATTACK_REGION_PUNCH);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_hitboxes();
        let mut edited = pristine.clone();
        edited[0].damage = 12.5;

        let (after, report) = rewrite_hitboxes(text, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 1, "only damage moved: {after}");
        // The bare `0` and `1` the parser decoded into const names stay bare.
        assert!(after.contains("1.0, 1.0, 0, 1, false"), "{after}");
        assert_eq!(after, text.replace("10.0, 361", "12.5, 361"));
    }

    /// A hitbox's frames are the block it sits in, not arguments of its call: a retime moves
    /// the call to the block on its new frame, carrying the values edited with it.
    #[test]
    fn retiming_a_hitbox_moves_its_call_with_its_new_values() {
        let text = r#"unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("top"), 10.0, 361, 100, 0, 30, 4.0, 0.0, 8.0, 0.0, None, None, None, 1.0, 1.0, 0, 1, false, 0, 0.0, 0, false, false, true, true, false, 0, 0, 0, false, Hash40::new("collision_attr_normal"), 0, 0, 0);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_hitboxes();
        let mut edited = pristine.clone();
        edited[0].active_start += 4;
        edited[0].damage = 12.0;

        let (after, report) = rewrite_hitboxes(text, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        let moved = crate::acmd::parse_acmd_script(&after).to_hitboxes();
        assert_eq!((moved[0].active_start, moved[0].damage), (9, 12.0), "{after}");
        assert!(after.contains("frame(agent.lua_state_agent, 9.0);"), "{after}");

        // Timing that runs through a loop is not restructured: the retime is reported, and the
        // damage it was bundled with still lands.
        let looped = text
            .replace("    frame(agent.lua_state_agent, 5.0);\n", "    for _ in 0..2 {\n    wait(agent.lua_state_agent, 5.0);\n")
            .replace("    }\n}\n", "    }\n    }\n}\n");
        let pristine = crate::acmd::parse_acmd_script(&looped).to_hitboxes();
        let mut edited = pristine.clone();
        edited[0].active_start += 1;
        edited[0].damage = 12.0;
        let (after, report) = rewrite_hitboxes(&looped, "t", &pristine, &edited).unwrap();
        assert_eq!(report.skipped.len(), 1, "{report:?}");
        assert!(report.skipped[0].contains("retimed") && report.skipped[0].contains("loop"), "{report:?}");
        assert!(after.contains("12.0, 361"), "{after}");
        assert!(after.contains("wait(agent.lua_state_agent, 5.0);"), "{after}");
    }

    /// kirby/ThrowHi's `ATTACK_IGNORE_THROW`, which the archive writes without the capsule
    /// options every `ATTACK` carries. The slot table has to follow the call it is aimed at.
    const CAPSULE_LESS: &str = r#"unsafe extern "C" fn game_throwhi(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 12.0);
    if macros::is_excute(agent) {
        macros::ATTACK_IGNORE_THROW(agent, 0, 0, Hash40::new("top"), 7.0, 65, 95, 0, 85, 9.5, 0.0, 6.5, 2.0, 1.0, 1.0, *ATTACK_SETOFF_KIND_OFF, *ATTACK_LR_CHECK_POS, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_M, *COLLISION_SOUND_ATTR_KICK, *ATTACK_REGION_BODY);
    }
}
"#;

    /// Retuning a call written without the capsule triple must count slots from the call,
    /// not from the macro's full signature. Off by three, the hitlag multiplier is written
    /// into the z offset and the setoff kind into hitlag — well-formed source, wrong move.
    #[test]
    fn a_capsule_less_call_is_retuned_through_its_own_shifted_slots() {
        let pristine = crate::acmd::parse_acmd_script(CAPSULE_LESS).to_hitboxes();
        assert_eq!(pristine.len(), 1);
        let mut edited = pristine.clone();
        edited[0].damage = 9.0;
        edited[0].hitlag_mult = 0.5;

        let (after, report) = rewrite_hitboxes(CAPSULE_LESS, "t", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 2, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        // Damage is ahead of the optional arguments and cannot shift.
        assert!(after.contains(r#"Hash40::new("top"), 9.0, 65,"#), "{after}");
        // Hitlag is the argument straight after the z offset here. The z offset keeps its
        // own value, which is what the unshifted table used to overwrite.
        assert!(
            after.contains("6.5, 2.0, 0.5, 1.0, *ATTACK_SETOFF"),
            "{after}"
        );
        // And the edit survives a read-back, which is the property that actually matters.
        assert_eq!(crate::acmd::parse_acmd_script(&after).to_hitboxes(), edited);
    }

    /// The capsule arguments are not there to write to. An edit that lands nowhere has to be
    /// named, not dropped — and must not fall through onto whatever argument sits at 13.
    #[test]
    fn a_capsule_edit_on_a_call_without_one_is_reported() {
        let pristine = crate::acmd::parse_acmd_script(CAPSULE_LESS).to_hitboxes();
        let mut edited = pristine.clone();
        edited[0].capsule_end = Some([1.0, 2.0, 3.0]);

        let (after, report) = rewrite_hitboxes(CAPSULE_LESS, "t", &pristine, &edited).unwrap();
        assert_eq!(after, CAPSULE_LESS, "nothing may be written");
        assert_eq!(report.changed, 0);
        assert!(
            report.skipped.iter().any(|s| s.contains("too short")),
            "{report:?}"
        );
    }

    /// The two `ATTACK`-family macros share the id space. Matching on id and part alone made
    /// a hitbox ambiguous between them — or worse, retuned the wrong one.
    #[test]
    fn an_attack_edit_never_lands_in_the_ignore_throw_call_beside_it() {
        let text = r#"unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("top"), 10.0, 361, 100, 0, 30, 4.0, 0.0, 8.0, 0.0, None, None, None, 1.0, 1.0, 0, 1, false, 0, 0.0, 0, false, false, true, true, false, 0, 0, 0, false, Hash40::new("collision_attr_normal"), 0, 0, 0);
    }
    frame(agent.lua_state_agent, 9.0);
    if macros::is_excute(agent) {
        macros::ATTACK_IGNORE_THROW(agent, 0, 0, Hash40::new("top"), 7.0, 65, 95, 0, 85, 9.5, 0.0, 6.5, 2.0, 1.0, 1.0, *ATTACK_SETOFF_KIND_OFF, *ATTACK_LR_CHECK_POS, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_M, *COLLISION_SOUND_ATTR_KICK, *ATTACK_REGION_BODY);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_hitboxes();
        assert_eq!(pristine.len(), 2);
        assert_eq!(
            (pristine[0].id, pristine[1].id),
            (0, 0),
            "the ids really do collide"
        );
        assert_eq!(pristine[0].func, "ATTACK");
        assert_eq!(pristine[1].func, "ATTACK_IGNORE_THROW");

        let mut edited = pristine.clone();
        edited[0].damage = 15.0;

        let (after, report) = rewrite_hitboxes(text, "t", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(
            after.contains(r#"Hash40::new("top"), 15.0, 361,"#),
            "{after}"
        );
        assert!(
            after.contains(r#"Hash40::new("top"), 7.0, 65,"#),
            "the throw-piercing hitbox must be untouched:\n{after}"
        );
    }

    /// Swapping which family member a hitbox is is a different macro, not a different
    /// value — the call would also have to gain or lose the capsule triple.
    #[test]
    fn changing_the_attack_macro_is_reported_rather_than_written() {
        let pristine = crate::acmd::parse_acmd_script(CAPSULE_LESS).to_hitboxes();
        let mut edited = pristine.clone();
        edited[0].func = "ATTACK".into();

        let (after, report) = rewrite_hitboxes(CAPSULE_LESS, "t", &pristine, &edited).unwrap();
        assert_eq!(after, CAPSULE_LESS);
        assert_eq!(report.changed, 0);
        assert!(
            report
                .skipped
                .iter()
                .any(|s| s.contains("macro being called")),
            "{report:?}"
        );
    }

    /// A wind box and an attack hitbox can carry the same id, and matching on id alone sent
    /// a wind edit into whichever `ATTACK` shared its number.
    #[test]
    fn a_wind_box_edit_never_lands_in_an_attack_call() {
        let text = r#"unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("top"), 10.0, 361, 100, 0, 30, 4.0, 0.0, 8.0, 0.0, None, None, None, 1.0, 1.0, 0, 1, false, 0, 0.0, 0, false, false, true, true, false, 0, 0, 0, false, Hash40::new("collision_attr_normal"), 0, 0, 0);
        macros::AREA_WIND_2ND_arg10(agent, 0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_hitboxes();
        let wind = pristine
            .iter()
            .position(|h| h.category == 2)
            .expect("a wind box");
        assert_eq!(
            pristine[wind].id, pristine[0].id,
            "the ids really do collide"
        );

        let mut edited = pristine.clone();
        let payload = edited[wind].wind.as_mut().expect("a wind payload");
        // Slot 5 is the wind area's X offset. Slot 5 of the `ATTACK` beside it is the angle.
        payload.args[5] = 77.0;

        let (after, report) = rewrite_hitboxes(text, "t", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(
            after.contains(
                "macros::ATTACK(agent, 0, 0, Hash40::new(\"top\"), 10.0, 361, 100, 0, 30, 4.0, \
                 0.0, 8.0, 0.0, None"
            ),
            "the ATTACK call must not move:\n{after}"
        );
        assert!(
            after.contains(
                "macros::AREA_WIND_2ND_arg10(agent, 0, 1.0, 2.0, 3.0, 4.0, 77, 6.0, 7.0, 8.0, \
                 9.0);"
            ),
            "{after}"
        );
    }

    const WIND: &str = r#"unsafe extern "C" fn game_specialn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::AREA_WIND_2ND_arg10(agent, 0, 1, 80, 300, 0.8, 4, 12, 24, 16, 50);
        macros::AREA_WIND_2ND_RAD(agent, 1, 0.5, 0.02, 1000, 1, -2, 6, 18);
    }
}
"#;

    /// The wind family's whole point: four commands that share slots 0..=7 and nothing else.
    /// Slot 8 is the rectangle's height and the radial call's lifetime, so retuning one
    /// through the other's table would change how long the area lives.
    #[test]
    fn a_wind_value_rewrites_only_its_own_argument() {
        let pristine = crate::acmd::parse_acmd_script(WIND).to_hitboxes();
        let mut edited = pristine.clone();
        let rect = edited
            .iter_mut()
            .find(|h| h.wind.as_ref().is_some_and(|w| !w.is_radial()))
            .expect("the rectangular wind");
        let payload = rect.wind.as_mut().unwrap();
        payload.args[1] = 2.5; // strength
        payload.args[8] = 20.0; // height — the radial call has a lifetime here

        let (after, report) = rewrite_hitboxes(WIND, "t", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 2, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(
            after.contains(
                "macros::AREA_WIND_2ND_arg10(agent, 0, 2.5, 80, 300, 0.8, 4, 12, 24, 20, 50);"
            ),
            "{after}"
        );
        assert!(
            after.contains("macros::AREA_WIND_2ND_RAD(agent, 1, 0.5, 0.02, 1000, 1, -2, 6, 18);"),
            "the radial call must be untouched:\n{after}"
        );
    }

    /// Rectangular and radial are different calls, not different values — the argument they
    /// disagree about is the shape itself. Writing one over the other is exactly the
    /// cross-family corruption the slot tables exist to prevent.
    #[test]
    fn changing_a_wind_from_rectangular_to_radial_is_reported_rather_than_written() {
        let pristine = crate::acmd::parse_acmd_script(WIND).to_hitboxes();
        let mut edited = pristine.clone();
        let rect = edited
            .iter_mut()
            .find(|h| h.wind.as_ref().is_some_and(|w| !w.is_radial()))
            .expect("the rectangular wind");
        let payload = rect.wind.as_mut().unwrap();
        payload.command = "AREA_WIND_2ND_RAD_arg9".into();
        payload.args.remove(8);

        let (after, report) = rewrite_hitboxes(WIND, "t", &pristine, &edited).unwrap();
        assert_eq!(after, WIND);
        assert_eq!(report.changed, 0);
        assert!(
            report
                .skipped
                .iter()
                .any(|s| s.contains("AREA_WIND_2ND_arg10") && s.contains("AREA_WIND_2ND_RAD_arg9")),
            "{report:?}"
        );
    }

    /// The arity is part of the command name and every argument is a bare float, so there is
    /// no shape to fall back on: a call whose length disagrees with its name is refused rather
    /// than retuned by position into whatever the author actually wrote there.
    #[test]
    fn a_wind_call_of_the_wrong_length_is_refused() {
        let short = WIND.replace(
            "macros::AREA_WIND_2ND_arg10(agent, 0, 1, 80, 300, 0.8, 4, 12, 24, 16, 50);",
            "macros::AREA_WIND_2ND_arg10(agent, 0, 1, 80, 300, 0.8, 4, 12, 24, 16);",
        );
        let pristine = crate::acmd::parse_acmd_script(WIND).to_hitboxes();
        let mut edited = pristine.clone();
        edited[0].wind.as_mut().unwrap().args[1] = 2.5;

        let (after, report) = rewrite_hitboxes(&short, "t", &pristine, &edited).unwrap();
        assert_eq!(after, short);
        assert_eq!(report.changed, 0);
        assert!(
            report.skipped.iter().any(|s| s.contains("not the 10 that")),
            "{report:?}"
        );
    }

    /// A wind area's end frame is its lifetime argument, so retiming it through the panel is a
    /// value edit and must land — while moving the bar on the timeline, which the payload
    /// cannot account for, must be reported.
    #[test]
    fn a_wind_lifetime_edit_is_written_but_a_timeline_retime_is_reported() {
        let pristine = crate::acmd::parse_acmd_script(WIND).to_hitboxes();

        let mut edited = pristine.clone();
        let start = edited[0].active_start;
        let payload = edited[0].wind.as_mut().unwrap();
        payload.args[9] = 30.0;
        edited[0].active_end = edited[0].wind.as_ref().unwrap().end_frame(start);
        let (after, report) = rewrite_hitboxes(WIND, "t", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(
            after.contains(
                "macros::AREA_WIND_2ND_arg10(agent, 0, 1, 80, 300, 0.8, 4, 12, 24, 16, 30);"
            ),
            "{after}"
        );

        let mut dragged = pristine.clone();
        dragged[0].active_end += 7;
        let (after, report) = rewrite_hitboxes(WIND, "t", &pristine, &dragged).unwrap();
        assert_eq!(after, WIND);
        assert_eq!(report.changed, 0);
        assert!(
            report.skipped.iter().any(|s| s.contains("was retimed")),
            "{report:?}"
        );
    }

    /// The shorter commands have no lifetime slot: the area runs until an
    /// `AreaModule::erase_wind` on a later line, so its end frame is not this call's to
    /// explain. Measuring it against the lifetime anyway reported a retime on every single
    /// edit, because a command with no lifetime "ends" at [`u32::MAX`].
    #[test]
    fn a_wind_ended_by_erase_wind_is_still_retunable() {
        let text = r#"unsafe extern "C" fn game_specialn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::AREA_WIND_2ND_RAD(agent, 1, 0.5, 0.02, 1000, 1, -2, 6, 18);
    }
    wait(agent.lua_state_agent, 10.0);
    if macros::is_excute(agent) {
        AreaModule::erase_wind(agent.module_accessor, 1);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_hitboxes();
        assert_eq!(pristine.len(), 1, "{pristine:#?}");
        assert_ne!(
            pristine[0].active_end,
            u32::MAX,
            "the erase_wind is what ends it"
        );

        let mut edited = pristine.clone();
        edited[0].wind.as_mut().unwrap().args[7] = 22.0; // radius

        let (after, report) = rewrite_hitboxes(text, "t", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(
            after.contains("macros::AREA_WIND_2ND_RAD(agent, 1, 0.5, 0.02, 1000, 1, -2, 6, 22);"),
            "{after}"
        );
    }

    /// `CATCH` numbers its arguments differently from `ATTACK` — slot 4 is the grab box's
    /// size, not damage — so a grab box is retuned through its own layout, never the attack
    /// one.
    #[test]
    fn a_grab_box_is_retuned_through_the_catch_layout() {
        let text = r#"unsafe extern "C" fn game_catch(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 8.0);
    if macros::is_excute(agent) {
        macros::CATCH(agent, 0, Hash40::new("top"), 5.5, 0.0, 6.4, 10.2, None, None, None, *FIGHTER_STATUS_KIND_SWALLOWED, *COLLISION_SITUATION_MASK_A);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_hitboxes();
        assert_eq!(pristine.len(), 1, "{pristine:#?}");
        assert_eq!(pristine[0].category, 1);

        let mut edited = pristine.clone();
        edited[0].size = 7.25;
        edited[0].offset_y = 9.0;
        edited[0].bone_name = "ArmL".into();
        edited[0].capsule_end = Some([1.0, 2.0, 3.0]);

        let (after, report) = rewrite_hitboxes(text, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");

        let round_tripped = crate::acmd::parse_acmd_script(&after).to_hitboxes();
        edited[0].bone_name = "arml".into();
        assert_eq!(round_tripped, edited, "\n{after}");
        // The two arguments no panel exposes are never touched.
        assert!(after.contains("*FIGHTER_STATUS_KIND_SWALLOWED"), "{after}");
        assert!(after.contains("*COLLISION_SITUATION_MASK_A"), "{after}");
    }

    /// A detection box is retuned through its own slots, in whichever shape it was written.
    ///
    /// The two calls here are the same box in the two forms the corpus uses, and the masks sit
    /// three slots apart between them. Editing the same property in both is what proves the
    /// tail is being located rather than assumed — a fixed slot number passes for one form and
    /// silently writes the situation mask over the hit status in the other.
    #[test]
    fn a_search_box_is_retuned_through_the_shape_it_was_written_in() {
        let text = r#"unsafe extern "C" fn game_search(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::SEARCH(agent, 0, 0, Hash40::new("top"), 4.0, 0.0, 7.0, 8.0, *COLLISION_KIND_MASK_ATTACK, *HIT_STATUS_MASK_ALL, 0, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_FIGHTER, *COLLISION_PART_MASK_ALL, false);
        macros::SEARCH(agent, 1, 0, Hash40::new("top"), 7.5, 0.0, 7.0, 4.0, 0.0, 7.0, 13.0, *COLLISION_KIND_MASK_ATTACK, *HIT_STATUS_MASK_NORMAL, 60, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_hitboxes();
        assert_eq!(pristine.len(), 2, "{pristine:#?}");

        let mut edited = pristine.clone();
        // The short-form box: edit a slot on each side of the absent capsule.
        edited[0].size = 5.25;
        edited[0].situation_mask = "COLLISION_SITUATION_MASK_G".into();
        edited[0].search.as_mut().unwrap().collision_kind = "COLLISION_KIND_MASK_GRAB".into();
        // The long-form box: the same two properties, three slots further along.
        edited[1].offset_z = 6.5;
        edited[1].category_mask = "COLLISION_CATEGORY_MASK_FIGHTER".into();
        edited[1].search.as_mut().unwrap().hit_status = "HIT_STATUS_MASK_XLU".into();

        let (after, report) = rewrite_hitboxes(text, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_hitboxes(),
            edited,
            "\n{after}"
        );

        // The two arguments no panel exposes are untouched in both shapes, and a property left
        // alone on one box keeps what it had even though the other box changed it.
        assert!(after.contains("*HIT_STATUS_MASK_ALL"), "{after}");
        assert!(after.contains("*COLLISION_KIND_MASK_ATTACK"), "{after}");
        assert!(
            after.contains(", 60, "),
            "the undocumented slot survives\n{after}"
        );
        assert!(after.contains("false);"), "{after}");
    }

    /// Giving a capsule to a search box with no capsule slots is reported, never written.
    ///
    /// Slot 8 holds `*COLLISION_KIND_MASK_ATTACK` in the short form. This is the same refusal
    /// `CATCH` makes, and it is here as its own test because the slot number differs — a fix
    /// applied to one family says nothing about the other.
    #[test]
    fn adding_a_capsule_to_a_short_form_search_is_refused() {
        let text = r#"unsafe extern "C" fn game_search(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::SEARCH(agent, 0, 0, Hash40::new("top"), 4.0, 0.0, 7.0, 8.0, *COLLISION_KIND_MASK_ATTACK, *HIT_STATUS_MASK_ALL, 0, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_FIGHTER, *COLLISION_PART_MASK_ALL, false);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_hitboxes();
        let mut edited = pristine.clone();
        edited[0].capsule_end = Some([1.0, 2.0, 3.0]);
        edited[0].size = 9.0;

        let (after, report) = rewrite_hitboxes(text, "t", &pristine, &edited).unwrap();
        assert!(
            after.contains("*COLLISION_KIND_MASK_ATTACK"),
            "what the box looks for must survive a refused capsule edit\n{after}"
        );
        assert!(!after.contains("Some(1.0)"), "{after}");
        assert!(
            report.skipped.iter().any(|s| s.contains("capsule end")),
            "{report:?}"
        );
        assert!(after.contains("Hash40::new(\"top\"), 9.0,"), "{after}");
    }

    /// A search box never retunes the size modifier that shares its name.
    ///
    /// `SET_SEARCH_SIZE_EXIST(agent, 0, 7)` names the same id. Matching sites by prefix, or
    /// pooling every collision into one candidate list, would write a bone hash into its size.
    #[test]
    fn a_search_edit_does_not_reach_the_search_size_modifier() {
        let text = r#"unsafe extern "C" fn game_search(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::SEARCH(agent, 0, 0, Hash40::new("top"), 4.0, 0.0, 7.0, 8.0, *COLLISION_KIND_MASK_ATTACK, *HIT_STATUS_MASK_ALL, 0, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_FIGHTER, *COLLISION_PART_MASK_ALL, false);
        macros::SET_SEARCH_SIZE_EXIST(agent, 0, 7);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_hitboxes();
        assert_eq!(pristine.len(), 1, "only the box is a box\n{pristine:#?}");
        let mut edited = pristine.clone();
        edited[0].size = 5.5;
        edited[0].bone_name = "ArmL".into();

        let (after, report) = rewrite_hitboxes(text, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(
            after.contains("macros::SET_SEARCH_SIZE_EXIST(agent, 0, 7);"),
            "the modifier is left exactly as it was\n{after}"
        );
        assert!(after.contains("Hash40::new(\"arml\"), 5.5,"), "{after}");
    }

    /// Giving a capsule to a grab that has no capsule slots is reported, never written.
    ///
    /// In the Lua-shaped form the three endpoint arguments are absent, so slots 7 and 8 hold
    /// the status kind and the situation mask. Writing `Some(1.0)` over them would silently
    /// turn Kirby's inhale into a grab that neither swallows nor compiles. Adding the capsule
    /// needs three arguments *inserted*, which a slot rewrite cannot do — so it is refused.
    ///
    /// The edits that do fit the call still land: refusing one property must not cost the rest.
    #[test]
    fn adding_a_capsule_to_a_grab_written_without_the_slots_is_refused() {
        // Verbatim from kirby/SpecialNStart.
        let text = r#"unsafe extern "C" fn game_catch(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 8.0);
    if macros::is_excute(agent) {
        macros::CATCH(agent, 0, Hash40::new("top"), 6.0, 0.0, 6.0, 5.0, *FIGHTER_STATUS_KIND_SWALLOWED, *COLLISION_SITUATION_MASK_GA);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_hitboxes();
        assert_eq!(pristine.len(), 1, "{pristine:#?}");
        assert_eq!(pristine[0].capsule_end, None);

        let mut edited = pristine.clone();
        edited[0].capsule_end = Some([1.0, 2.0, 3.0]);
        edited[0].size = 7.25;

        let (after, report) = rewrite_hitboxes(text, "t", &pristine, &edited).unwrap();
        assert!(
            after.contains("*FIGHTER_STATUS_KIND_SWALLOWED")
                && after.contains("*COLLISION_SITUATION_MASK_GA"),
            "the grab's own behaviour must survive a refused capsule edit\n{after}"
        );
        assert!(!after.contains("Some(1.0)"), "{after}");
        assert!(
            report.skipped.iter().any(|s| s.contains("capsule end")),
            "the refusal has to be visible, not silent: {report:?}"
        );
        // The size still lands.
        assert!(after.contains("Hash40::new(\"top\"), 7.25,"), "{after}");
    }

    /// A grab box carries attack-only fields that `CATCH` has no argument for. Editing one
    /// must land nowhere rather than in whichever slot happens to share its number.
    #[test]
    fn a_damage_edit_on_a_grab_box_writes_nothing() {
        let text = r#"unsafe extern "C" fn game_catch(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 8.0);
    if macros::is_excute(agent) {
        macros::CATCH(agent, 0, Hash40::new("top"), 5.5, 0.0, 6.4, 10.2, None, None, None, 0, 0);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_hitboxes();
        let mut edited = pristine.clone();
        edited[0].damage = 42.0;

        let (after, report) = rewrite_hitboxes(text, "t", &pristine, &edited).unwrap();
        assert_eq!(after, text, "a CATCH has no damage argument to write into");
        assert_eq!(report.changed, 0, "{report:?}");
    }

    /// The old failure: a grab box matched an `ATTACK` by id alone and had the attack
    /// layout written into it.
    #[test]
    fn a_grab_box_edit_never_lands_in_an_attack_call() {
        let text = r#"unsafe extern "C" fn game_test(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("top"), 10.0, 361, 100, 0, 30, 4.0, 0.0, 8.0, 0.0, None, None, None, 1.0, 1.0, 0, 1, false, 0, 0.0, 0, false, false, true, true, false, 0, 0, 0, false, Hash40::new("collision_attr_normal"), 0, 0, 0);
    }
}
"#;
        // A grab box sharing the attack's id, with no CATCH anywhere in the script.
        let pristine = vec![crate::data::Hitbox {
            id: 0,
            part: 0,
            category: 1,
            ..Default::default()
        }];
        let mut edited = pristine.clone();
        edited[0].size = 9.0;

        let (after, report) = rewrite_hitboxes(text, "t", &pristine, &edited).unwrap();
        assert_eq!(after, text, "the ATTACK call must not move");
        assert_eq!(report.changed, 0);
        assert_eq!(report.skipped.len(), 1, "{report:?}");
    }

    #[test]
    fn an_attack_fp_edit_uses_its_own_source_slots() {
        let text = r#"unsafe extern "C" fn game_fp(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::ATTACK_FP(agent, 2, 3, Hash40::new("top"), 9.5, 45, 100, 2, 30, 4.0, 1.0, 2.0, 3.0, Hash40::new("collision_attr_fire"), 0.25, 0.5, 0.75, true, false, 0, 3, 4, 0, true, 7, 8, false, 9, false, true, false, false, 10, true, false, *ATTACK_LR_CHECK_POS, false, true, false, false, false, 12);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_hitboxes();
        assert_eq!(pristine.len(), 1, "{pristine:#?}");
        assert_eq!(pristine[0].category, crate::data::CAT_ATTACK_FP);
        let mut edited = pristine.clone();
        edited[0].damage = 14.25;
        edited[0].lr_check = "ATTACK_LR_CHECK_F".into();
        edited[0].collision_attr = "collision_attr_elec".into();
        edited[0].is_reflectable = true;

        let (after, report) = rewrite_hitboxes(text, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 4, "{report:?}");
        assert!(
            after.contains(
                "Hash40::new(\"top\"), 14.25, 45, 100, 2, 30, 4.0, 1.0, 2.0, 3.0, Hash40::new(\"collision_attr_elec\")"
            ),
            "{after}"
        );
        assert!(after.contains("*ATTACK_LR_CHECK_F, false, true"), "{after}");
        assert!(
            crate::acmd::parse_acmd_script(&after).to_hitboxes()[0].is_reflectable,
            "{after}"
        );
    }

    #[test]
    fn set_speed_ex_write_back_changes_only_velocity_arguments() {
        let text = r#"unsafe extern "C" fn game_speed(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::SET_SPEED_EX(agent, 0, -3.8, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_speed_ex_events();
        let mut edited = pristine.clone();
        edited[0].call.speed_x = 1.25;
        edited[0].call.speed_y = 2.0;
        let (after, report) = rewrite_speed_ex(text, "mario/speed", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 2, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains(
            "macros::SET_SPEED_EX(agent, 1.25, 2, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);"
        ));
        assert!(after.contains("frame(agent.lua_state_agent, 4.0);"));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_speed_ex_events(),
            edited
        );
    }

    #[test]
    fn set_speed_write_back_changes_only_direct_velocity_arguments() {
        let text = r#"unsafe extern "C" fn game_speed(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::SET_SPEED(agent, 0, -3.8);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_speed_events();
        let mut edited = pristine.clone();
        edited[0].call.speed_x = 1.25;
        edited[0].call.speed_y = 2.0;
        let (after, report) = rewrite_speed(text, "mario/speed", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 2, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains("macros::SET_SPEED(agent, 1.25, 2);"));
        assert!(after.contains("frame(agent.lua_state_agent, 4.0);"));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_speed_events(),
            edited
        );
    }

    #[test]
    fn generated_set_speed_helper_write_back_round_trips_without_touching_definition() {
        let source = r#"unsafe extern "C" fn game_speed(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::SET_SPEED(agent, 0, -3.8);
    }
}
"#;
        let script = crate::acmd::parse_acmd_script(source);
        let generated = crate::acmd::preview_game_fn(&script, "speed");
        let pristine = crate::acmd::parse_acmd_script(&generated).to_speed_events();
        assert_eq!(speed_sites(&generated).len(), 1, "{generated}");
        assert!(
            generated.contains("unsafe fn visionary_set_speed(")
                && generated.contains("visionary_set_speed(agent, 0.0, -3.8);"),
            "{generated}"
        );

        let mut edited = pristine.clone();
        edited[0].call.speed_x = 1.25;
        edited[0].call.speed_y = 2.0;
        let (after, report) = rewrite_speed(&generated, "mario/speed", &pristine, &edited).unwrap();

        assert_eq!(report.changed, 2, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains(
            "unsafe fn visionary_set_speed(agent: &mut L2CAgentBase, speed_x: f32, speed_y: f32)"
        ));
        assert!(after.contains("visionary_set_speed(agent, 1.25, 2);"));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_speed_events(),
            edited
        );
    }

    #[test]
    fn malformed_set_speed_shapes_block_positional_source_sync() {
        let text = r#"unsafe extern "C" fn game_speed(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::SET_SPEED(agent, 0);
        macros::SET_SPEED(agent, 0, 1, 2);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_speed_events();
        assert!(pristine.is_empty());
        let (after, report) = rewrite_speed(text, "mario/speed", &pristine, &pristine).unwrap();
        assert_eq!(after, text);
        assert!(report.skipped.is_empty());
        assert_eq!(speed_sites(text).len(), 0);
    }

    #[test]
    fn speed_addition_and_correction_write_back_change_only_their_values() {
        let text = r#"unsafe extern "C" fn game_speed(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::ADD_SPEED_NO_LIMIT(agent, 0, -3.8);
        macros::CORRECT(agent, *GROUND_CORRECT_KIND_GROUND);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text);
        let speed_before = pristine.to_add_speed_no_limit_events();
        let correct_before = pristine.to_correct_events();
        let mut speed_after = speed_before.clone();
        speed_after[0].call.speed_x = 1.25;
        speed_after[0].call.speed_y = 2.0;
        let mut correct_after = correct_before.clone();
        correct_after[0].call.kind = "2".into();

        let (after_speed, speed_report) =
            rewrite_add_speed_no_limit(text, "mario/speed", &speed_before, &speed_after).unwrap();
        assert_eq!(speed_report.changed, 2, "{speed_report:?}");
        assert!(speed_report.skipped.is_empty(), "{speed_report:?}");
        let (after, correct_report) =
            rewrite_correct(&after_speed, "mario/speed", &correct_before, &correct_after).unwrap();
        assert_eq!(correct_report.changed, 1, "{correct_report:?}");
        assert!(correct_report.skipped.is_empty(), "{correct_report:?}");
        assert!(after.contains("macros::ADD_SPEED_NO_LIMIT(agent, 1.25, 2);"));
        assert!(after.contains("macros::CORRECT(agent, 2);"));
        assert!(after.contains("frame(agent.lua_state_agent, 4.0);"));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_add_speed_no_limit_events(),
            speed_after
        );
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_correct_events(),
            correct_after
        );
    }

    #[test]
    fn ft_catch_stop_write_back_changes_only_numeric_values() {
        let text = r#"unsafe extern "C" fn game_throwf(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        macros::FT_CATCH_STOP(agent, 6, 1);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_ft_catch_stop_events();
        let mut edited = pristine.clone();
        edited[0].call.arg1 = 7.5;
        edited[0].call.arg2 = 0.25;

        let (after, report) =
            rewrite_ft_catch_stop(text, "mario/throw_f", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 2, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains("macros::FT_CATCH_STOP(agent, 7.5, 0.25);"));
        assert!(after.contains("frame(agent.lua_state_agent, 6.0);"));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_ft_catch_stop_events(),
            edited
        );
    }

    #[test]
    fn malformed_ft_catch_stop_shapes_block_positional_source_sync() {
        let text = r#"unsafe extern "C" fn game_throwf(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::FT_CATCH_STOP(agent, 6);
        macros::FT_CATCH_STOP(agent, 6, 1, 2);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_ft_catch_stop_events();
        assert!(pristine.is_empty());
        assert!(ft_catch_stop_sites(text).is_empty());
        let (after, report) = rewrite_ft_catch_stop(text, "mario/throw_f", &[], &[]).unwrap();
        assert_eq!(after, text);
        assert!(
            report.skipped.is_empty(),
            "unchanged opaque calls need no warning"
        );
    }

    #[test]
    fn ft_start_adjust_motion_frame_write_back_changes_only_numeric_value() {
        let text = r#"unsafe extern "C" fn game_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 17.0);
    macros::FT_START_ADJUST_MOTION_FRAME_arg1(agent, 0.85);
}
"#;
        let pristine =
            crate::acmd::parse_acmd_script(text).to_ft_start_adjust_motion_frame_events();
        let mut edited = pristine.clone();
        edited[0].call.value = 1.25;

        let (after, report) = rewrite_ft_start_adjust_motion_frame(
            text,
            "bayonetta/attack_air_n",
            &pristine,
            &edited,
        )
        .unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains("macros::FT_START_ADJUST_MOTION_FRAME_arg1(agent, 1.25);"));
        assert!(after.contains("frame(agent.lua_state_agent, 17.0);"));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_ft_start_adjust_motion_frame_events(),
            edited
        );
    }

    #[test]
    fn revised_ft_start_adjust_motion_frame_shapes_block_positional_source_sync() {
        let text = r#"unsafe extern "C" fn game_specialn(agent: &mut L2CAgentBase) {
    FT_START_ADJUST_MOTION_FRAME_REVISED_arg1(1.0);
}
"#;
        let pristine =
            crate::acmd::parse_acmd_script(text).to_ft_start_adjust_motion_frame_events();
        assert!(pristine.is_empty());
        assert!(ft_start_adjust_motion_frame_sites(text).is_empty());
        let (after, report) =
            rewrite_ft_start_adjust_motion_frame(text, "trail/special_n", &[], &[]).unwrap();
        assert_eq!(after, text);
        assert!(report.skipped.is_empty(), "{report:?}");
    }

    #[test]
    fn clr_speed_source_sync_rewrites_only_the_authored_kinetic_token() {
        let text = r#"unsafe extern "C" fn game_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::CLR_SPEED(agent, *FIGHTER_KINETIC_ENERGY_ID_GRAVITY);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_clr_speed_events();
        let mut edited = pristine.clone();
        edited[0].call.kinetic_kind = "*FIGHTER_KINETIC_ENERGY_ID_MOTION".into();
        let (after, report) =
            rewrite_clr_speed(text, "mario/attack_air_n", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains("macros::CLR_SPEED(agent, *FIGHTER_KINETIC_ENERGY_ID_MOTION);"));
        assert!(after.contains("frame(agent.lua_state_agent, 4.0);"));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_clr_speed_events(),
            edited
        );
    }

    #[test]
    fn change_kinetic_source_sync_rewrites_only_the_authored_type_token() {
        let text = r#"unsafe extern "C" fn game_escapeair(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 19.0);
    if macros::is_excute(agent) {
        KineticModule::change_kinetic(agent.module_accessor, *FIGHTER_KINETIC_TYPE_FALL);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_change_kinetic_events();
        let mut edited = pristine.clone();
        edited[0].call.kinetic_type = "*FIGHTER_KINETIC_TYPE_GROUND".into();
        let (after, report) =
            rewrite_change_kinetic(text, "kirby/escape_air", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains(
            "KineticModule::change_kinetic(agent.module_accessor, *FIGHTER_KINETIC_TYPE_GROUND);"
        ));
        assert!(after.contains("frame(agent.lua_state_agent, 19.0);"));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_change_kinetic_events(),
            edited
        );
    }

    #[test]
    fn change_kinetic_source_sync_rewrites_the_measured_boma_shape() {
        let text = r#"unsafe extern "C" fn game_passiveceil(agent: &mut L2CAgentBase) {
    let boma = agent.boma();
    frame(agent.lua_state_agent, 18.0);
    if is_excute(agent) {
        KineticModule::change_kinetic(boma, *FIGHTER_KINETIC_TYPE_PASSIVE_CEIL);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_change_kinetic_events();
        let mut edited = pristine.clone();
        edited[0].call.kinetic_type = "*FIGHTER_KINETIC_TYPE_FALL".into();
        let (after, report) =
            rewrite_change_kinetic(text, "bayonetta/passive_ceil", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains("KineticModule::change_kinetic(boma, *FIGHTER_KINETIC_TYPE_FALL);"));
        assert!(after.contains("let boma = agent.boma();"));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_change_kinetic_events(),
            edited
        );
    }

    #[test]
    fn kinetic_add_speed_source_sync_rewrites_only_the_vector_and_keeps_receiver() {
        let text = r#"unsafe extern "C" fn game_escapeair(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 1.0);
    if macros::is_excute(agent) {
        KineticModule::add_speed(agent.module_accessor, &Vector3f{x: 0.72, y: -1.5, z: 0.0});
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_kinetic_add_speed_events();
        let mut edited = pristine.clone();
        edited[0].call.speed_x = 1.25;
        edited[0].call.speed_y = 2.0;
        let (after, report) =
            rewrite_kinetic_add_speed(text, "mario/escape_air", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains(
            "KineticModule::add_speed(agent.module_accessor, &Vector3f{x: 1.25, y: 2.0, z: 0.0});"
        ));
        assert!(after.contains("frame(agent.lua_state_agent, 1.0);"));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_kinetic_add_speed_events(),
            edited
        );
    }

    #[test]
    fn kinetic_add_speed_source_sync_rewrites_the_measured_boma_shape() {
        let text = r#"unsafe extern "C" fn game_escapeair(agent: &mut L2CAgentBase) {
    let boma = agent.boma();
    frame(agent.lua_state_agent, 1.0);
    if is_excute(agent) {
        KineticModule::add_speed(boma, &Vector3f{x: 0.0, y: 0.8, z: 0.0});
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_kinetic_add_speed_events();
        let mut edited = pristine.clone();
        edited[0].call.speed_y = 1.33;
        let (after, report) =
            rewrite_kinetic_add_speed(text, "mario/escape_air", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(
            after.contains("KineticModule::add_speed(boma, &Vector3f{x: 0.0, y: 1.33, z: 0.0});")
        );
        assert!(after.contains("let boma = agent.boma();"));
    }

    #[test]
    fn malformed_kinetic_add_speed_vectors_are_source_only() {
        let text = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        KineticModule::add_speed(agent.module_accessor, &Vector3f{x: 1.0, y: 2.0, z: 1.0});
    }
}
"#;
        assert!(crate::acmd::parse_acmd_script(text)
            .to_kinetic_add_speed_events()
            .is_empty());
        assert!(kinetic_add_speed_sites(text).is_empty());
        let (after, report) = rewrite_kinetic_add_speed(text, "mario/x", &[], &[]).unwrap();
        assert_eq!(after, text);
        assert!(report.skipped.is_empty(), "{report:?}");
    }

    #[test]
    fn work_flag_source_sync_rewrites_only_authored_tokens_and_keeps_receivers() {
        let text = r#"unsafe extern "C" fn game_attackairn(agent: &mut L2CAgentBase) {
    let boma = agent.boma();
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_ATTACK_FLAG_START_SMASH_HOLD);
        WorkModule::off_flag(boma, 17);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_work_flag_events();
        let mut edited = pristine.clone();
        edited[0].call.flag = "23".into();
        edited[1].call.flag = "*FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO".into();

        let (after, report) =
            rewrite_work_flags(text, "mario/attack_air_n", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 2, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains("WorkModule::on_flag(agent.module_accessor, 23);"));
        assert!(
            after.contains("WorkModule::off_flag(boma, *FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO);")
        );
        assert!(after.contains("let boma = agent.boma();"));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_work_flag_events(),
            edited
        );
    }

    #[test]
    fn work_flag_source_sync_refuses_retiming_operation_changes_and_malformed_tokens() {
        let text = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        WorkModule::on_flag(agent.module_accessor, 7);
        WorkModule::off_flag(agent.module_accessor, 8);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_work_flag_events();
        let mut edited = pristine.clone();
        edited[0].frame = 2;
        edited[1].call.action = crate::data::WorkFlagAction::On;
        edited[1].call.flag = "make_flag()".into();
        let (after, report) = rewrite_work_flags(text, "mario/x", &pristine, &edited).unwrap();
        assert_eq!(after, text);
        assert_eq!(report.changed, 0, "{report:?}");
        assert_eq!(report.skipped.len(), 2, "{report:?}");
    }

    #[test]
    fn work_transition_term_source_sync_rewrites_only_authored_tokens_and_keeps_receivers() {
        let text = r#"unsafe extern "C" fn game_attackairn(agent: &mut L2CAgentBase) {
    let boma = agent.boma();
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        WorkModule::enable_transition_term(agent.module_accessor, *FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN);
        WorkModule::unable_transition_term(boma, 17);
        WorkModule::enable_transition_term_group(agent.module_accessor, *FIGHTER_STATUS_TRANSITION_GROUP_CHK_AIR_LANDING);
        WorkModule::unable_transition_term_group_ex(agent.module_accessor, 18);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_work_transition_term_events();
        let mut edited = pristine.clone();
        edited[0].call.transition_term = "23".into();
        edited[1].call.transition_term = "*FIGHTER_STATUS_TRANSITION_TERM_ID_ENABLE_COMBO".into();
        edited[2].call.transition_term = "19".into();
        edited[3].call.transition_term =
            "*FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_HI".into();

        let (after, report) =
            rewrite_work_transition_terms(text, "mario/attack_air_n", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 4, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains("WorkModule::enable_transition_term(agent.module_accessor, 23);"));
        assert!(after.contains(
            "WorkModule::unable_transition_term(boma, *FIGHTER_STATUS_TRANSITION_TERM_ID_ENABLE_COMBO);"
        ));
        assert!(
            after.contains("WorkModule::enable_transition_term_group(agent.module_accessor, 19);")
        );
        assert!(after.contains(
            "WorkModule::unable_transition_term_group_ex(agent.module_accessor, *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_HI);"
        ));
        assert!(after.contains("let boma = agent.boma();"));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_work_transition_term_events(),
            edited
        );
    }

    #[test]
    fn work_transition_term_source_sync_refuses_retiming_operation_changes_and_malformed_tokens() {
        let text = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        WorkModule::enable_transition_term(agent.module_accessor, 7);
        WorkModule::unable_transition_term(agent.module_accessor, 8);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_work_transition_term_events();
        let mut edited = pristine.clone();
        edited[0].frame = 2;
        edited[1].call.action = crate::data::WorkTransitionTermAction::Enable;
        edited[1].call.transition_term = "make_term()".into();
        let (after, report) =
            rewrite_work_transition_terms(text, "mario/x", &pristine, &edited).unwrap();
        assert_eq!(after, text);
        assert_eq!(report.changed, 0, "{report:?}");
        assert_eq!(report.skipped.len(), 2, "{report:?}");
    }

    #[test]
    fn work_transition_term_source_sync_never_rewrites_or_normalizes_receivers() {
        let text = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        WorkModule::enable_transition_term(agent.module_accessor, *FIGHTER_STATUS_TRANSITION_TERM_ID_DASH_TO_RUN);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_work_transition_term_events();
        let mut receiver_changed = pristine.clone();
        receiver_changed[0].call.receiver = crate::data::WorkTransitionTermReceiver::Boma;
        receiver_changed[0].call.transition_term = "23".into();
        let (after_receiver_edit, report) =
            rewrite_work_transition_terms(text, "mario/x", &pristine, &receiver_changed).unwrap();
        assert_eq!(after_receiver_edit, text);
        assert_eq!(report.changed, 0, "{report:?}");
        assert_eq!(report.skipped.len(), 1, "{report:?}");

        let changed_source = text.replace("agent.module_accessor", "boma");
        let mut term_changed = pristine.clone();
        term_changed[0].call.transition_term = "23".into();
        let (after_source_receiver_edit, report) =
            rewrite_work_transition_terms(&changed_source, "mario/x", &pristine, &term_changed)
                .unwrap();
        assert_eq!(after_source_receiver_edit, changed_source);
        assert_eq!(report.changed, 0, "{report:?}");
        assert_eq!(report.skipped.len(), 1, "{report:?}");
    }

    #[test]
    fn work_module_set_source_sync_rewrites_value_and_slot_tokens() {
        let text = r#"unsafe extern "C" fn game_itemgrasspull(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        WorkModule::set_int(agent.module_accessor, *FIGHTER_ITEM_GRASS_PULL_STEP_PICKUP, *FIGHTER_STATUS_ITEM_GRASS_PULL_WORK_INT_NEXT_STEP);
    }
    frame(agent.lua_state_agent, 47.0);
    if macros::is_excute(agent) {
        WorkModule::set_float(agent.module_accessor, 5.0, *FIGHTER_INSTANCE_WORK_ID_FLOAT_FINISH_CAMERA_THROW_RAY_LENGTH);
    }
    frame(agent.lua_state_agent, 70.0);
    if macros::is_excute(agent) {
        WorkModule::set_int64(boma, hash40("fall_damage") as i64, FIGHTER_STATUS_FINAL_WORK_INT_REQUEST_LOOP_DAMAGE_MOTION);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_work_module_set_events();
        let mut edited = pristine.clone();
        edited[0].call.value = "4".into();
        edited[0].call.slot = "9".into();
        edited[1].call.value = "6.25".into();
        edited[1].call.slot = "12".into();
        edited[2].call.value = "1099511627777".into();
        edited[2].call.slot = "14".into();

        let (after, report) =
            rewrite_work_module_sets(text, "kirby/item_grass_pull", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 6, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains("WorkModule::set_int(agent.module_accessor, 4, 9);"));
        assert!(after.contains("WorkModule::set_float(agent.module_accessor, 6.25, 12);"));
        assert!(after.contains("WorkModule::set_int64(boma, 1099511627777, 14);"));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_work_module_set_events(),
            edited
        );
    }

    #[test]
    fn item_grass_pull_source_sync_changes_one_step_value_and_preserves_order_and_receiver() {
        let text = r#"unsafe extern "C" fn game_itemgrasspull(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        WorkModule::set_int(agent.module_accessor, *FIGHTER_ITEM_GRASS_PULL_STEP_PICKUP, *FIGHTER_STATUS_ITEM_GRASS_PULL_WORK_INT_NEXT_STEP);
    }
    frame(agent.lua_state_agent, 12.0);
    if macros::is_excute(agent) {
        WorkModule::set_int(agent.module_accessor, *FIGHTER_ITEM_GRASS_PULL_STEP_PULL, *FIGHTER_STATUS_ITEM_GRASS_PULL_WORK_INT_NEXT_STEP);
    }
    frame(agent.lua_state_agent, 15.0);
    if macros::is_excute(agent) {
        WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_ITEM_GRASS_PULL_FLAG_STTUS_END_IF_PICKUP_FAILED);
        WorkModule::set_int(agent.module_accessor, 4, *FIGHTER_STATUS_ITEM_GRASS_PULL_WORK_INT_THROW_N_OFFSET_ANIM_FRAME_COUNT_MAX);
    }
    frame(agent.lua_state_agent, 18.0);
    if macros::is_excute(agent) {
        WorkModule::set_int(agent.module_accessor, *FIGHTER_ITEM_GRASS_PULL_STEP_HOIST, *FIGHTER_STATUS_ITEM_GRASS_PULL_WORK_INT_NEXT_STEP);
    }
    frame(agent.lua_state_agent, 30.0);
    if macros::is_excute(agent) {
        WorkModule::set_int(agent.module_accessor, *FIGHTER_ITEM_GRASS_PULL_STEP_END, *FIGHTER_STATUS_ITEM_GRASS_PULL_WORK_INT_NEXT_STEP);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_work_module_set_events();
        assert_eq!(
            pristine.iter().map(|event| event.frame).collect::<Vec<_>>(),
            vec![5, 12, 15, 18, 30]
        );
        assert!(pristine
            .iter()
            .all(|event| event.call.receiver == "agent.module_accessor"));
        let mut edited = pristine.clone();
        edited[1].call.value =
            crate::move_nodes::native_work::ITEM_GRASS_PULL_STEP_VALUES[0].into();

        let (after, report) =
            rewrite_work_module_sets(text, "cloud/item_grass_pull", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(
            after,
            text.replace(
                "WorkModule::set_int(agent.module_accessor, *FIGHTER_ITEM_GRASS_PULL_STEP_PULL",
                "WorkModule::set_int(agent.module_accessor, *FIGHTER_ITEM_GRASS_PULL_STEP_PICKUP"
            )
        );
        assert!(after.contains(
            "WorkModule::on_flag(agent.module_accessor, *FIGHTER_STATUS_ITEM_GRASS_PULL_FLAG_STTUS_END_IF_PICKUP_FAILED);"
        ));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_work_module_set_events(),
            edited
        );

        let mut edited_receiver = pristine.clone();
        edited_receiver[1].call.receiver = "boma".into();
        let (unchanged, report) =
            rewrite_work_module_sets(text, "cloud/item_grass_pull", &pristine, &edited_receiver)
                .unwrap();
        assert_eq!(unchanged, text);
        assert_eq!(report.changed, 0, "{report:?}");
        assert_eq!(report.skipped.len(), 1, "{report:?}");
        assert!(report.skipped[0].contains("changed receiver"), "{report:?}");

        let receiver_drift = text.replace(
            "WorkModule::set_int(agent.module_accessor, *FIGHTER_ITEM_GRASS_PULL_STEP_PULL",
            "WorkModule::set_int(boma, *FIGHTER_ITEM_GRASS_PULL_STEP_PULL",
        );
        let (unchanged, report) =
            rewrite_work_module_sets(&receiver_drift, "cloud/item_grass_pull", &pristine, &edited)
                .unwrap();
        assert_eq!(unchanged, receiver_drift);
        assert_eq!(report.changed, 0, "{report:?}");
        assert_eq!(report.skipped.len(), 1, "{report:?}");
    }

    #[test]
    fn work_module_set_source_sync_refuses_retiming_kind_and_malformed_edits() {
        let text = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        WorkModule::set_int(agent.module_accessor, 1, 7);
        WorkModule::set_float(agent.module_accessor, 2.0, 8);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_work_module_set_events();
        let mut edited = pristine.clone();
        edited[0].frame = 2;
        edited[1].call.kind = crate::data::WorkModuleSetKind::Int;
        edited[1].call.value = "make_value()".into();
        let (after, report) =
            rewrite_work_module_sets(text, "mario/x", &pristine, &edited).unwrap();
        assert_eq!(after, text);
        assert_eq!(report.changed, 0, "{report:?}");
        assert_eq!(report.skipped.len(), 2, "{report:?}");
    }

    #[test]
    fn work_module_inc_int_source_sync_rewrites_authored_slots_for_standard_and_hdr() {
        let standard = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        WorkModule::inc_int(agent.module_accessor, *FIGHTER_STATUS_WORK_INT_NEXT_STEP);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(standard).to_work_module_inc_int_events();
        let mut edited = pristine.clone();
        edited[0].call.slot = "9".into();
        let (after, report) =
            rewrite_work_module_inc_int(standard, "mario/x", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains("WorkModule::inc_int(agent.module_accessor, 9);"));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_work_module_inc_int_events(),
            edited
        );

        let hdr = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    let boma = agent.boma();
    frame(agent.lua_state_agent, 5.0);
    if is_excute(agent) {
        WorkModule::inc_int(boma, *FIGHTER_STATUS_WORK_INT_NEXT_STEP);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(hdr).to_work_module_inc_int_events();
        let mut edited = pristine.clone();
        edited[0].call.slot = "*FIGHTER_STATUS_WORK_INT_OTHER".into();
        let (after, report) =
            rewrite_work_module_inc_int(hdr, "mario/x_hdr", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains("WorkModule::inc_int(boma, *FIGHTER_STATUS_WORK_INT_OTHER);"));
        assert!(after.contains("let boma = agent.boma();"));
    }

    #[test]
    fn work_module_inc_int_source_sync_refuses_retiming_and_malformed_edits() {
        let text = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        WorkModule::inc_int(agent.module_accessor, 7);
        WorkModule::inc_int(agent.module_accessor, 8);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_work_module_inc_int_events();
        let mut edited = pristine.clone();
        edited[0].frame = 2;
        edited[1].call.slot = "make_slot()".into();
        let (after, report) =
            rewrite_work_module_inc_int(text, "mario/x", &pristine, &edited).unwrap();
        assert_eq!(after, text);
        assert_eq!(report.changed, 0, "{report:?}");
        assert_eq!(report.skipped.len(), 2, "{report:?}");
    }

    #[test]
    fn kinetic_clear_speed_all_source_sync_retimes_flat_standard_and_hdr_calls() {
        let standard = r#"unsafe extern "C" fn game_escapeair(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        KineticModule::clear_speed_all(agent.module_accessor);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(standard).to_kinetic_clear_speed_all_events();
        let mut edited = pristine.clone();
        edited[0].frame = 6;
        let (after, report) =
            rewrite_kinetic_clear_speed_all(standard, "mario/escape_air", &pristine, &edited)
                .unwrap();
        assert_eq!(report.changed, 2, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains("frame(agent.lua_state_agent, 6.0);"));
        assert!(after.contains("KineticModule::clear_speed_all(agent.module_accessor);"));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_kinetic_clear_speed_all_events(),
            edited
        );

        let hdr = r#"unsafe extern "C" fn game_escapeair(agent: &mut L2CAgentBase) {
    let boma = agent.boma();
    frame(agent.lua_state_agent, 4.0);
    if is_excute(agent) {
        KineticModule::clear_speed_all(boma);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(hdr).to_kinetic_clear_speed_all_events();
        let mut edited = pristine.clone();
        edited[0].frame = 9;
        let (after, report) =
            rewrite_kinetic_clear_speed_all(hdr, "mario/escape_air_hdr", &pristine, &edited)
                .unwrap();
        assert_eq!(report.changed, 2, "{report:?}");
        assert!(after.contains("let boma = agent.boma();"));
        assert!(after.contains("KineticModule::clear_speed_all(boma);"));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_kinetic_clear_speed_all_events(),
            edited
        );
    }

    #[test]
    fn malformed_kinetic_clear_speed_all_shapes_are_source_only() {
        let text = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        KineticModule::clear_speed_all(agent.module_accessor, 1);
        KineticModule::clear_speed_all(other.module_accessor);
    }
}
"#;
        assert!(crate::acmd::parse_acmd_script(text)
            .to_kinetic_clear_speed_all_events()
            .is_empty());
        assert!(kinetic_clear_speed_all_sites(text).is_empty());
        let (after, report) = rewrite_kinetic_clear_speed_all(text, "mario/x", &[], &[]).unwrap();
        assert_eq!(after, text);
        assert!(report.skipped.is_empty(), "{report:?}");
    }

    #[test]
    fn kinetic_set_consider_ground_friction_source_sync_retimes_standard_and_hdr_points() {
        let standard = r#"unsafe extern "C" fn game_escapeair(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        KineticModule::set_consider_ground_friction(agent.module_accessor, false, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(standard)
            .to_kinetic_set_consider_ground_friction_events();
        let mut edited = pristine.clone();
        edited[0].frame = 6;
        edited[0].call.consider_ground_friction = true;
        edited[0].call.kinetic_energy_attribute = "7".into();
        let (after, report) = rewrite_kinetic_set_consider_ground_friction(
            standard,
            "mario/escape_air",
            &pristine,
            &edited,
        )
        .unwrap();
        assert_eq!(report.changed, 2, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains("frame(agent.lua_state_agent, 6.0);"));
        assert!(after.contains(
            "KineticModule::set_consider_ground_friction(agent.module_accessor, true, 7);"
        ));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_kinetic_set_consider_ground_friction_events(),
            edited
        );

        let hdr = standard.replace("agent.module_accessor", "boma");
        let pristine =
            crate::acmd::parse_acmd_script(&hdr).to_kinetic_set_consider_ground_friction_events();
        let mut edited = pristine.clone();
        edited[0].call.consider_ground_friction = true;
        let (after, report) = rewrite_kinetic_set_consider_ground_friction(
            &hdr,
            "mario/escape_air_hdr",
            &pristine,
            &edited,
        )
        .unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(after.contains("KineticModule::set_consider_ground_friction(boma, true"));
        assert!(after.contains("let boma = agent.boma();") || !after.contains("agent.boma"));

        let mixed = r#"unsafe extern "C" fn game_escapeair(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        KineticModule::set_consider_ground_friction(agent.module_accessor, false, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);
    }
    frame(agent.lua_state_agent, 8.0);
    if macros::is_excute(agent) {
        KineticModule::set_consider_ground_friction(agent.module_accessor, true, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);
    }
}
"#;
        let pristine =
            crate::acmd::parse_acmd_script(mixed).to_kinetic_set_consider_ground_friction_events();
        let mut edited = vec![pristine[0].clone(), pristine[1].clone()];
        edited[1].call.kinetic_energy_attribute = "9".into();
        edited.insert(
            1,
            crate::data::KineticSetConsiderGroundFrictionEvent {
                frame: 6,
                call: crate::data::KineticSetConsiderGroundFrictionCall {
                    consider_ground_friction: false,
                    kinetic_energy_attribute: "5".into(),
                },
                site: 1,
            },
        );
        edited[2].site = 2;
        let (after, report) = rewrite_kinetic_set_consider_ground_friction(
            mixed,
            "mario/escape_air",
            &pristine,
            &edited,
        )
        .unwrap();
        assert_eq!(report.changed, 2, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains(
            "KineticModule::set_consider_ground_friction(agent.module_accessor, false, 5);"
        ));
        assert!(after.contains(
            "KineticModule::set_consider_ground_friction(agent.module_accessor, true, 9);"
        ));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_kinetic_set_consider_ground_friction_events(),
            edited
        );
    }

    #[test]
    fn malformed_kinetic_set_consider_ground_friction_source_shapes_are_source_only() {
        let text = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        KineticModule::set_consider_ground_friction(agent.module_accessor, 1, 0);
        KineticModule::set_consider_ground_friction(other, false, 0);
    }
}
"#;
        assert!(crate::acmd::parse_acmd_script(text)
            .to_kinetic_set_consider_ground_friction_events()
            .is_empty());
        assert!(kinetic_set_consider_ground_friction_sites(text).is_empty());
        let (after, report) =
            rewrite_kinetic_set_consider_ground_friction(text, "mario/x", &[], &[]).unwrap();
        assert_eq!(after, text);
        assert!(report.skipped.is_empty(), "{report:?}");
    }

    #[test]
    fn kinetic_energy_source_sync_rewrites_ids_and_keeps_operation_and_receiver() {
        let text = r#"unsafe extern "C" fn game_attackairlw(agent: &mut L2CAgentBase) {
    let boma = agent.boma();
    frame(agent.lua_state_agent, 1.0);
    if is_excute(agent) {
        KineticModule::suspend_energy(boma, *FIGHTER_KINETIC_ENERGY_ID_CONTROL);
        KineticModule::resume_energy(boma, *FIGHTER_KINETIC_ENERGY_ID_CONTROL);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_kinetic_energy_events();
        let mut edited = pristine.clone();
        edited[0].call.kinetic_energy_id = "*FIGHTER_KINETIC_ENERGY_ID_GRAVITY".into();
        let (after, report) =
            rewrite_kinetic_energy(text, "mario/attack_air_lw", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after
            .contains("KineticModule::suspend_energy(boma, *FIGHTER_KINETIC_ENERGY_ID_GRAVITY);"));
        assert!(after
            .contains("KineticModule::resume_energy(boma, *FIGHTER_KINETIC_ENERGY_ID_CONTROL);"));
        assert!(after.contains("let boma = agent.boma();"));
    }

    #[test]
    fn malformed_kinetic_energy_source_shapes_are_source_only() {
        let text = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        KineticModule::suspend_energy(agent.module_accessor);
    }
}
"#;
        assert!(crate::acmd::parse_acmd_script(text)
            .to_kinetic_energy_events()
            .is_empty());
        assert!(kinetic_energy_sites(text).is_empty());
        let (after, report) = rewrite_kinetic_energy(text, "mario/x", &[], &[]).unwrap();
        assert_eq!(after, text);
        assert!(report.skipped.is_empty(), "{report:?}");
    }

    #[test]
    fn kinetic_energy_source_sync_accepts_enable_and_unable_operations() {
        let text = r#"unsafe extern "C" fn game_specialhi(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        KineticModule::enable_energy(agent.module_accessor, *FIGHTER_KINETIC_ENERGY_ID_GRAVITY);
        KineticModule::unable_energy(agent.module_accessor, *FIGHTER_KINETIC_ENERGY_ID_CONTROL);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_kinetic_energy_events();
        let mut edited = pristine.clone();
        edited[1].call.kinetic_energy_id = "*FIGHTER_KINETIC_ENERGY_ID_GRAVITY".into();
        let (after, report) =
            rewrite_kinetic_energy(text, "mario/special_hi", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains(
            "KineticModule::enable_energy(agent.module_accessor, *FIGHTER_KINETIC_ENERGY_ID_GRAVITY);"
        ));
        assert!(after.contains(
            "KineticModule::unable_energy(agent.module_accessor, *FIGHTER_KINETIC_ENERGY_ID_GRAVITY);"
        ));
    }

    #[test]
    fn set_air_source_sync_handles_flat_remove_retime_and_insert() {
        let text = r#"unsafe extern "C" fn game_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::SET_AIR(agent);
    }
    frame(agent.lua_state_agent, 8.0);
    if macros::is_excute(agent) {
        macros::SET_AIR(agent);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_set_air_events();
        let edited = vec![crate::data::SetAirEvent { frame: 6, site: 0 }];
        let (after, report) =
            rewrite_set_air(text, "mario/attack_air_n", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 3, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        let round_trip = crate::acmd::parse_acmd_script(&after).to_set_air_events();
        assert_eq!(round_trip, edited, "{after}");
        assert!(
            after.contains("frame(agent.lua_state_agent, 6.0);"),
            "{after}"
        );
    }

    #[test]
    fn set_air_source_sync_refuses_branch_restructuring() {
        let text = r#"unsafe extern "C" fn game_specialn(agent: &mut L2CAgentBase) {
    if WorkModule::is_flag(agent.module_accessor, 1) {
        if macros::is_excute(agent) {
            macros::SET_AIR(agent);
        }
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_set_air_events();
        let edited = Vec::new();
        let (after, report) = rewrite_set_air(text, "mario/special_n", &pristine, &edited).unwrap();
        assert_eq!(after, text);
        assert!(report
            .skipped
            .iter()
            .any(|note| note.contains("loop/branch")));
    }

    #[test]
    fn malformed_set_speed_ex_shapes_block_positional_source_sync() {
        let text = r#"unsafe extern "C" fn game_speed(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::SET_SPEED_EX(agent, 0, 1);
        macros::SET_SPEED_EX(agent, 0, 1, 0, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_speed_ex_events();
        assert!(pristine.is_empty());
        let (after, report) = rewrite_speed_ex(text, "mario/speed", &pristine, &pristine).unwrap();
        assert_eq!(after, text);
        assert!(
            report.skipped.is_empty(),
            "unchanged opaque calls need no warning"
        );
        assert_eq!(speed_ex_sites(text).len(), 0);
    }

    #[test]
    fn malformed_speed_addition_and_correction_shapes_block_positional_sync() {
        let text = r#"unsafe extern "C" fn game_speed(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        macros::ADD_SPEED_NO_LIMIT(agent, 0);
        macros::ADD_SPEED_NO_LIMIT(agent, 0, 1, 2);
        macros::CORRECT(agent);
        macros::CORRECT(agent, 1, 2);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text);
        assert!(pristine.to_add_speed_no_limit_events().is_empty());
        assert!(pristine.to_correct_events().is_empty());
        assert_eq!(add_speed_no_limit_sites(text).len(), 0);
        assert_eq!(correct_sites(text).len(), 0);
    }

    #[test]
    fn motion_module_set_rate_source_sync_changes_only_numeric_values() {
        let text = r#"unsafe extern "C" fn game_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        MotionModule::set_rate(agent.module_accessor, 0.8);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_motion_module_set_rate_events();
        let mut edited = pristine.clone();
        edited[0].call.rate = 1.25;
        let (after, report) =
            rewrite_motion_module_set_rate(text, "mario/attack_air_n", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains("MotionModule::set_rate(agent.module_accessor, 1.25);"));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_motion_module_set_rate_events(),
            edited
        );
    }

    #[test]
    fn motion_module_set_rate_source_sync_refuses_retime_and_malformed_sites() {
        let text = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        MotionModule::set_rate(agent.module_accessor, 0.8);
        MotionModule::set_rate(agent.module_accessor, -378992935);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(text).to_motion_module_set_rate_events();
        assert_eq!(pristine.len(), 1);
        let mut edited = pristine.clone();
        edited[0].frame = 4;
        let (after, report) =
            rewrite_motion_module_set_rate(text, "mario/x", &pristine, &edited).unwrap();
        assert_eq!(after, text);
        assert!(report.skipped.iter().any(|note| note.contains("retimed")));
        assert_eq!(motion_module_set_rate_sites(text).len(), 1);
    }

    #[test]
    fn motion_module_set_rate_partial_source_sync_changes_only_numeric_rate() {
        let text = r#"unsafe extern "C" fn game_attackairn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        MotionModule::set_rate_partial(agent.module_accessor, *FIGHTER_MOTION_PART_SET_KIND_UPPER_BODY, 0.8);
    }
}
"#;
        let pristine =
            crate::acmd::parse_acmd_script(text).to_motion_module_set_rate_partial_events();
        let mut edited = pristine.clone();
        edited[0].call.rate = 1.25;
        let (after, report) =
            rewrite_motion_module_set_rate_partial(text, "mario/attack_air_n", &pristine, &edited)
                .unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains(
            "MotionModule::set_rate_partial(agent.module_accessor, *FIGHTER_MOTION_PART_SET_KIND_UPPER_BODY, 1.25);"
        ));
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_motion_module_set_rate_partial_events(),
            edited
        );
    }

    #[test]
    fn expression_motion_module_set_rate_partial_source_sync_uses_expression_category() {
        let text = r#"unsafe extern "C" fn expression_appeallwl(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 8.0);
    if macros::is_excute(agent) {
        MotionModule::set_rate_partial(agent.module_accessor, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 0.5);
    }
}
"#;
        let script = crate::acmd::parse_expression_script(text);
        let pristine = script.to_motion_module_set_rate_partial_events();
        let mut edited = pristine.clone();
        edited[0].call.rate = 1.25;
        let (after, report) =
            rewrite_motion_module_set_rate_partial(text, "pacman/appeal_lw_l", &pristine, &edited)
                .unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(after.contains(
            "MotionModule::set_rate_partial(agent.module_accessor, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 1.25);"
        ));
        assert_eq!(
            crate::acmd::parse_expression_script(&after).to_motion_module_set_rate_partial_events(),
            edited
        );
    }

    #[test]
    fn motion_module_set_rate_partial_source_sync_refuses_structure_part_changes_and_malformed_sites(
    ) {
        let text = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        MotionModule::set_rate_partial(boma, *FIGHTER_MOTION_PART_SET_KIND_UPPER_BODY, 0.8);
        MotionModule::set_rate_partial(boma, *FIGHTER_MOTION_PART_SET_KIND_UPPER_BODY);
    }
}
"#;
        let pristine =
            crate::acmd::parse_acmd_script(text).to_motion_module_set_rate_partial_events();
        assert_eq!(pristine.len(), 1);
        let mut edited = pristine.clone();
        edited[0].call.part_kind = "*FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL".into();
        let (after, report) =
            rewrite_motion_module_set_rate_partial(text, "mario/x", &pristine, &edited).unwrap();
        assert_eq!(after, text);
        assert!(report.skipped.iter().any(|note| note.contains("part kind")));
        assert_eq!(motion_module_set_rate_partial_sites(text).len(), 1);
    }

    #[test]
    fn motion_module_set_frame_partial_source_sync_changes_only_seek_frame_and_sync() {
        let text = r#"unsafe extern "C" fn expression_appeallwl(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 8.0);
    if macros::is_excute(agent) {
        MotionModule::set_frame_partial(agent.module_accessor, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 2);
    }
    frame(agent.lua_state_agent, 18.0);
    if macros::is_excute(agent) {
        MotionModule::set_frame_partial(boma, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 0, false);
    }
}
"#;
        let pristine =
            crate::acmd::parse_expression_script(text).to_motion_module_set_frame_partial_events();
        assert_eq!(pristine.len(), 2);
        let mut edited = pristine.clone();
        edited[0].call.frame = 5.0;
        edited[1].call.sync = Some(true);
        let (after, report) =
            rewrite_motion_module_set_frame_partial(text, "pacman/appeal_lw_l", &pristine, &edited)
                .unwrap();
        assert_eq!(report.changed, 2, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");

        // Only the two argument spans move: the receiver spelling, the part-kind token and the
        // three-argument call's missing `sync` are all left exactly as the author wrote them.
        assert!(after.contains(
            "MotionModule::set_frame_partial(agent.module_accessor, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 5);"
        ));
        assert!(after.contains(
            "MotionModule::set_frame_partial(boma, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 0, true);"
        ));
        assert_eq!(
            crate::acmd::parse_expression_script(&after)
                .to_motion_module_set_frame_partial_events(),
            edited
        );
    }

    #[test]
    fn motion_module_set_frame_partial_source_sync_refuses_arity_part_and_retime_changes() {
        let text = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        MotionModule::set_frame_partial(boma, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL, 2);
        MotionModule::set_frame_partial(boma, *FIGHTER_PACMAN_MOTION_PART_SET_KIND_MATERIAL);
    }
}
"#;
        let pristine =
            crate::acmd::parse_acmd_script(text).to_motion_module_set_frame_partial_events();
        assert_eq!(pristine.len(), 1);
        assert_eq!(motion_module_set_frame_partial_sites(text).len(), 1);

        // Spelling out the boolean the game was supplying is an arity change, not a value edit.
        let mut edited = pristine.clone();
        edited[0].call.sync = Some(true);
        let (after, report) =
            rewrite_motion_module_set_frame_partial(text, "pacman/x", &pristine, &edited).unwrap();
        assert_eq!(after, text);
        assert!(
            report.skipped.iter().any(|note| note.contains("`sync`")),
            "{report:?}"
        );

        let mut edited = pristine.clone();
        edited[0].call.part_kind = "*FIGHTER_MOTION_PART_SET_KIND_UPPER_BODY".into();
        let (after, report) =
            rewrite_motion_module_set_frame_partial(text, "pacman/x", &pristine, &edited).unwrap();
        assert_eq!(after, text);
        assert!(
            report.skipped.iter().any(|note| note.contains("part kind")),
            "{report:?}"
        );

        let mut edited = pristine.clone();
        edited[0].frame += 1;
        let (after, report) =
            rewrite_motion_module_set_frame_partial(text, "pacman/x", &pristine, &edited).unwrap();
        assert_eq!(after, text);
        assert!(
            report.skipped.iter().any(|note| note.contains("retimed")),
            "{report:?}"
        );
    }

    #[test]
    fn motion_module_set_helper_calculation_source_sync_changes_only_boolean_values() {
        let text = r#"unsafe extern "C" fn game_specialairhi(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 3.0);
    if macros::is_excute(agent) {
        MotionModule::set_helper_calculation(agent.module_accessor, false);
    }
}
"#;
        let pristine =
            crate::acmd::parse_acmd_script(text).to_motion_module_set_helper_calculation_events();
        let mut edited = pristine.clone();
        edited[0].call.enabled = true;
        let (after, report) = rewrite_motion_module_set_helper_calculation(
            text,
            "pichu/special_air_hi",
            &pristine,
            &edited,
        )
        .unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(
            after.contains("MotionModule::set_helper_calculation(agent.module_accessor, true);")
        );
        assert_eq!(
            crate::acmd::parse_acmd_script(&after).to_motion_module_set_helper_calculation_events(),
            edited
        );
    }

    #[test]
    fn motion_module_set_helper_calculation_source_sync_refuses_retime_and_malformed_sites() {
        let text = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    if macros::is_excute(agent) {
        MotionModule::set_helper_calculation(boma, false);
        MotionModule::set_helper_calculation(boma, enabled);
    }
}
"#;
        let pristine =
            crate::acmd::parse_acmd_script(text).to_motion_module_set_helper_calculation_events();
        assert_eq!(pristine.len(), 1);
        let mut edited = pristine.clone();
        edited[0].frame = 4;
        let (after, report) =
            rewrite_motion_module_set_helper_calculation(text, "pichu/x", &pristine, &edited)
                .unwrap();
        assert_eq!(after, text);
        assert!(report.skipped.iter().any(|note| note.contains("retimed")));
        assert_eq!(motion_module_set_helper_calculation_sites(text).len(), 1);
    }

    fn put(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A status registered on a chained `Agent::new`, whose body holds the article's motion. The
    /// chain form is synthetic; the status body shape (`sv_kinetic_energy!` in a `Main` status) is
    /// King-Boo-Moveset · src/kingteresa/weapon/fireball/status.rs:49 (tier 1). The parameter-agent
    /// layout King Boo really uses is `indexes_parameter_agent_status_through_install_module`.
    #[test]
    fn indexes_weapon_status_on_chain() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(
            root,
            "status.rs",
            r#"unsafe extern "C" fn shoot_main(weapon: &mut L2CWeaponCommon) -> L2CValue {
    sv_kinetic_energy!(set_speed, weapon, WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL, 1.5, 0.0);
    0.into()
}
pub fn install() {
    Agent::new("dedede_fireball").status(Main, *WEAPON_KINGTERESA_FIREBALL_STATUS_KIND_SHOOT, shoot_main).install();
}
"#,
        );
        let index = SourceIndex::build(root).unwrap();
        let bodies = index.status_bodies("dedede_fireball");
        assert_eq!(bodies.len(), 1, "{:?}", index.statuses);
        let (site, body) = &bodies[0];
        assert_eq!(site.phase, "Main");
        assert_eq!(site.kind_expr, "*WEAPON_KINGTERESA_FIREBALL_STATUS_KIND_SHOOT");
        assert_eq!(site.function, "shoot_main");
        assert!(body.contains("sv_kinetic_energy!"), "{body}");
    }

    /// A bound agent, a function in another file reached by path, the long phase spelling and
    /// `on_start`.
    /// Shy-Guy-Moveset · plugin/src/shyguy/articles/trampoline/status/mod.rs:16 and mod.rs:46 (tier 1)
    #[test]
    fn indexes_bound_agent_status() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(
            root,
            "jackbox.rs",
            "pub unsafe extern \"C\" fn jackbox_main(weapon: &mut L2CWeaponCommon) -> L2CValue { 0.into() }
",
        );
        put(
            root,
            "install.rs",
            r#"unsafe extern "C" fn trampoline_start(weapon: &mut L2CWeaponCommon) {}
unsafe extern "C" fn trampoline_init(weapon: &mut L2CWeaponCommon) -> L2CValue { 0.into() }
pub fn install() {
    let agent = &mut Agent::new("pacman_trampoline");
    agent.status(Init, 0, trampoline_init);
    agent.status(smashline::Main, *WEAPON_PACMAN_TRAMPOLINE_STATUS_KIND_JACKBOX, jackbox::jackbox_main);
    agent.on_start(trampoline_start);
    agent.install();
}
"#,
        );
        let index = SourceIndex::build(root).unwrap();
        let sites = &index.statuses["pacman_trampoline"];
        assert_eq!(sites.len(), 3, "{sites:?}");
        assert!(sites.iter().any(|s| s.phase == "Init" && s.kind_expr == "0" && s.function == "trampoline_init"));
        assert!(sites.iter().any(|s| s.phase == "Main" && s.function == "jackbox_main"));
        assert!(sites.iter().any(|s| s.phase == "Start" && s.kind_expr.is_empty()));
        assert_eq!(index.status_bodies("pacman_trampoline").len(), 3);
    }

    /// King Boo's real layout: `mod.rs` builds the agent and hands it to `status::install(agent)`;
    /// `status.rs` registers on the parameter. `start.rs` does the same with `on_start`.
    /// King-Boo-Moveset · src/kingteresa/weapon/fireball/mod.rs:5-11, status.rs:103-111;
    /// weapon/portrait/start.rs:26 (tier 1)
    #[test]
    fn indexes_parameter_agent_status_through_install_module() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(
            root,
            "weapon/fireball/mod.rs",
            r#"mod status;
mod start;
pub fn install() {
    let agent = &mut smashline::Agent::new("dedede_fireball");
    status::install(agent);
    start::install(agent);
    agent.install();
}
"#,
        );
        put(
            root,
            "weapon/fireball/status.rs",
            r#"unsafe extern "C" fn shoot_main(weapon: &mut L2CWeaponCommon) -> L2CValue {
    sv_kinetic_energy!(set_speed, weapon, WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL, 1.5, 0.0);
    0.into()
}
pub fn install(agent: &mut smashline::Agent) {
    agent.status(Main, FIREBALL_STATUS_KIND_SHOOT, shoot_main);
}
"#,
        );
        put(
            root,
            "weapon/fireball/start.rs",
            r#"unsafe extern "C" fn fireball_start(weapon: &mut L2CWeaponCommon) {}
pub fn install(agent: &mut smashline::Agent) {
    agent.on_start(fireball_start);
}
"#,
        );
        let index = SourceIndex::build(root).unwrap();
        let bodies = index.status_bodies("dedede_fireball");
        assert_eq!(bodies.len(), 2, "{:?} / {:?}", index.statuses, index.unresolved_statuses);
        assert!(bodies.iter().any(|(site, body)| site.phase == "Main" && body.contains("sv_kinetic_energy!")));
        assert!(bodies.iter().any(|(site, _)| site.phase == "Start" && site.function == "fireball_start"));
        assert!(index.unresolved_statuses.is_empty(), "{:?}", index.unresolved_statuses);
    }

    /// King-Boo-Moveset · src/kingteresa/mod.rs:47-52 hands `Agent::new("dedede")` to
    /// `status::install(agent)`, and src/kingteresa/status/status_catched_portrait.rs:75 in that
    /// module builds `Agent::new("fighter")` for a status every fighter gets (tier 1, smashline2).
    /// The module's parameter registrations are still dedede's; the generic agent keeps its own.
    #[test]
    fn a_generic_fighter_agent_does_not_claim_the_module() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(
            root,
            "kingteresa/mod.rs",
            "mod status;\npub fn do_install() {\n    let agent = &mut smashline::Agent::new(\"dedede\");\n    status::install(agent);\n    agent.install();\n}\n",
        );
        put(
            root,
            "kingteresa/status/mod.rs",
            "mod status_attack_air;\nmod status_catched_portrait;\npub fn install(agent: &mut smashline::Agent) {\n    status_attack_air::install(agent);\n    status_catched_portrait::install(agent);\n}\n",
        );
        put(
            root,
            "kingteresa/status/status_attack_air.rs",
            "unsafe extern \"C\" fn kingteresa_attack_air_status_pre(fighter: &mut L2CFighterCommon) -> L2CValue {\n    0.into()\n}\npub fn install(agent: &mut smashline::Agent) {\n    agent.status(Pre, *FIGHTER_STATUS_KIND_ATTACK_AIR, kingteresa_attack_air_status_pre);\n}\n",
        );
        put(
            root,
            "kingteresa/status/status_catched_portrait.rs",
            "unsafe extern \"C\" fn catched_pre(fighter: &mut L2CFighterCommon) -> L2CValue {\n    0.into()\n}\npub fn install(_agent: &mut smashline::Agent) {\n    Agent::new(\"fighter\")\n        .status(Pre, FIGHTER_STATUS_KIND_CATCHED_KINGTERESA_PORTRAIT, catched_pre)\n        .install();\n}\n",
        );
        let index = SourceIndex::build(root).unwrap();
        assert!(index.unresolved_statuses.is_empty(), "{:?}", index.unresolved_statuses);
        let dedede = index.status_bodies("dedede");
        assert_eq!(dedede.len(), 1, "{:?}", index.statuses);
        assert_eq!(dedede[0].0.function, "kingteresa_attack_air_status_pre");
        assert_eq!(index.status_bodies("fighter")[0].0.function, "catched_pre");
    }

    /// Sceptile-Moveset · plugin/src/sceptile/articles/bullet_seed/status/mod.rs:19 registers
    /// `.status(End, *WEAPON_SIMON_AXE_STATUS_KIND_FLY, fly::bullet_seed_end)`, and
    /// `bullet_seed_end` is defined both in `status/fly.rs:248` and in `bullet_seed/mod.rs:24`
    /// (tier 1, smashline2). The path names the one in `fly`.
    #[test]
    fn a_module_path_picks_the_function_in_that_module() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(
            root,
            "bullet_seed/mod.rs",
            "mod status;\nunsafe extern \"C\" fn bullet_seed_end(weapon: &mut L2CWeaponCommon) {}\n",
        );
        put(
            root,
            "bullet_seed/status/mod.rs",
            "mod fly;\npub fn install() {\n    Agent::new(\"mario_bulletseed\")\n        .status(End,  *WEAPON_SIMON_AXE_STATUS_KIND_FLY, fly::bullet_seed_end)\n        .install();\n}\n",
        );
        put(
            root,
            "bullet_seed/status/fly.rs",
            "pub(super) unsafe extern \"C\" fn bullet_seed_end(weapon: &mut L2CWeaponCommon) -> L2CValue {\n    0.into()\n}\n",
        );
        let index = SourceIndex::build(root).unwrap();
        assert!(index.unresolved_statuses.is_empty(), "{:?}", index.unresolved_statuses);
        let bodies = index.status_bodies("mario_bulletseed");
        assert_eq!(bodies.len(), 1);
        assert!(bodies[0].0.file.ends_with("fly.rs"), "{:?}", bodies[0].0.file);
        assert!(bodies[0].1.contains("-> L2CValue"));
    }

    /// Sceptile-Moveset · plugin/src/common/util.rs:1187-1191 defines `noop_status!` as
    /// `$( $agent.status($phase, $kind, $crate::empty_status_weapon); )+` (tier 1, smashline2).
    /// The template is not a registration; each invocation is listed as a no-op.
    #[test]
    fn a_macro_rules_body_is_not_a_registration() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(
            root,
            "util.rs",
            "#[macro_export]\nmacro_rules! noop_status {\n    ($agent:expr, $kind:expr, [$($phase:expr),+ $(,)?]) => {\n        $( $agent.status($phase, $kind, $crate::empty_status_weapon); )+\n    };\n}\npub fn install() {\n    let agent = &mut Agent::new(\"mario_bulletseed\");\n    noop_status!(agent, *WEAPON_SIMON_AXE_STATUS_KIND_HAVED, [Pre, Main]);\n}\n",
        );
        let index = SourceIndex::build(root).unwrap();
        let unresolved = &index.unresolved_statuses;
        assert_eq!(unresolved.len(), 2, "{unresolved:?}");
        assert!(unresolved.iter().all(|u| u.reason.contains("noop_status!") && u.kind_expr.starts_with('*')));
    }

    /// Closures, `noop_status!` (Shy Guy has eight) and a parameter nobody installs are not
    /// sites, but they are listed with a reason rather than dropped.
    /// Shy-Guy-Moveset · plugin/src/shyguy/articles/trampoline/status/mod.rs:13-14 (tier 1)
    #[test]
    fn indexes_unresolved_statuses_are_listed_with_reasons() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(
            root,
            "tramp/install.rs",
            r#"pub fn install() {
    let agent = &mut Agent::new("pacman_trampoline");
    noop_status!(agent, *WEAPON_PACMAN_TRAMPOLINE_STATUS_KIND_START, [Pre, Main, End]);
    agent.status(Main, 0, |weapon| 0.into());
    agent.status(End, 0, missing_function);
}
"#,
        );
        put(
            root,
            "orphan.rs",
            "pub fn install(agent: &mut Agent) {\n    agent.status(Main, 1, orphan_main);\n}\n",
        );
        let index = SourceIndex::build(root).unwrap();
        let unresolved = &index.unresolved_statuses;
        let noops = unresolved.iter().filter(|u| u.reason.contains("noop_status!")).collect::<Vec<_>>();
        assert_eq!(noops.len(), 3, "{unresolved:?}");
        assert!(noops.iter().all(|u| u.agent_hint.as_deref() == Some("pacman_trampoline")));
        assert!(unresolved.iter().any(|u| u.reason.contains("closure") && u.phase == "Main" && u.line == 4));
        assert!(unresolved.iter().any(|u| u.reason.contains("not defined") && u.phase == "End"));
        assert!(unresolved.iter().any(|u| u.kind_expr == "1" && u.reason.contains("parameter")), "{unresolved:?}");
        assert!(index.statuses.is_empty(), "{:?}", index.statuses);
    }

    /// The fighter's own status registrations never leak onto its article, or the reverse.
    #[test]
    fn indexes_fighter_status_not_on_article() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(
            root,
            "status.rs",
            r#"unsafe extern "C" fn fighter_main(fighter: &mut L2CFighterCommon) -> L2CValue { 0.into() }
unsafe extern "C" fn article_main(weapon: &mut L2CWeaponCommon) -> L2CValue { 0.into() }
pub fn install() {
    Agent::new("pacman").status(Main, *FIGHTER_STATUS_KIND_ATTACK, fighter_main).install();
    Agent::new("pacman_trampoline").status(Main, 0, article_main).install();
}
"#,
        );
        let index = SourceIndex::build(root).unwrap();
        let article = &index.statuses["pacman_trampoline"];
        assert_eq!(article.len(), 1);
        assert_eq!(article[0].function, "article_main");
        let fighter = &index.statuses["pacman"];
        assert_eq!(fighter.len(), 1);
        assert_eq!(fighter[0].function, "fighter_main");
    }
}

/// Layouts and spellings taken from real movesets (see `moveset_survey`), each of which the
/// indexer or the write-back used to get wrong.
#[cfg(test)]
mod moveset_layout_tests {
    use super::*;

    fn put(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// Empoleon (over Falco): one folder registers the fighter and two of its articles, each on
    /// its own `Agent::new` chain. The folder naming several agents used to drop every script
    /// in it, so the whole moveset read as vanilla Falco from the mirror.
    #[test]
    fn chained_registrations_attribute_each_script_to_its_own_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(root, "Cargo.toml", "[package]\nname = \"empoleon\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n");
        put(root, "src/lib.rs", "mod emperte;\n");
        put(root, "src/emperte/mod.rs", "mod acmd;\npub fn install() { acmd::install(); }\n");
        put(root, "src/emperte/acmd/mod.rs", "mod normals;\nmod specials;\npub fn install() { normals::install(); specials::install(); }\n");
        put(
            root,
            "src/emperte/acmd/normals.rs",
            r#"unsafe extern "C" fn game_attack11(agent: &mut L2CAgentBase) {}
pub fn install() {
    Agent::new("falco")
    .set_costume(costume.to_vec())
    .acmd("game_attack11", game_attack11, Priority::Default)
    .install();
}
"#,
        );
        put(
            root,
            "src/emperte/acmd/specials.rs",
            r#"unsafe extern "C" fn game_specialsstart(agent: &mut L2CAgentBase) {}
unsafe extern "C" fn game_fly(agent: &mut L2CAgentBase) {}
unsafe extern "C" fn ava_game_regular(agent: &mut L2CAgentBase) {}
pub fn install() {
    Agent::new("falco")
    .acmd("game_specialsstart", game_specialsstart, Priority::Default)
    .acmd("game_specialairsstart", game_specialsstart, Priority::Default)
    .install();

    Agent::new("falco_cannon")
    .acmd("game_fly", game_fly, Priority::Default)
    .install();

    let ava = &mut Agent::new("FALCO_ava");
    ava.game_acmd("game_regular", ava_game_regular, Priority::Default);
    ava.install();
}
"#,
        );
        let index = SourceIndex::build(root).unwrap();
        for script in ["game_attack11", "game_specialsstart", "game_specialairsstart"] {
            assert!(index.script("falco", script).is_some(), "falco/{script}: {:?}", index.fighters.keys());
        }
        assert!(index.script("falco_cannon", "game_fly").is_some());
        assert!(index.script("falco", "game_fly").is_none(), "an article script is not the fighter's");
        assert!(index.script("falco_ava", "game_regular").is_some());
        assert_eq!(index.conflict_count(), 0, "{:?}", index.conflicts);

        // A move it leaves vanilla, and a fighter it is not for, say which.
        let other = index.why_no_script("mario", "attack_11");
        assert!(other.contains("no `mario` scripts") && other.contains("falco"), "{other}");
        let unmodded = index.why_no_script("falco", "attack_lw4");
        assert!(unmodded.contains("does not define falco/attack_lw4"), "{unmodded}");
    }

    /// Two agents registering the same script name in one file are two scripts, not a
    /// replacement: smashline keys registrations by agent.
    #[test]
    fn same_script_on_two_agents_in_one_file_is_not_a_replacement() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(
            root,
            "acmd.rs",
            r#"unsafe extern "C" fn fly_a(agent: &mut L2CAgentBase) {}
unsafe extern "C" fn fly_b(agent: &mut L2CAgentBase) {}
pub fn install() {
    Agent::new("falco_blaster_bullet").acmd("game_fly", fly_a, Priority::Low).install();
    Agent::new("falco_cannon").acmd("game_fly", fly_b, Priority::Low).install();
}
"#,
        );
        let index = SourceIndex::build(root).unwrap();
        assert_eq!(index.script("falco_blaster_bullet", "game_fly").map(|s| s.function.as_str()), Some("fly_a"));
        assert_eq!(index.script("falco_cannon", "game_fly").map(|s| s.function.as_str()), Some("fly_b"));
    }

    /// Point `VISIONARY_PROBE_SOURCE` at a real project to see how it indexes:
    /// `cargo test probe_linked_source -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn probe_linked_source() {
        let Some(root) = std::env::var_os("VISIONARY_PROBE_SOURCE") else {
            return;
        };
        //NOCARGO
        let index = SourceIndex::build(Path::new(&root)).unwrap();
        let mut fighters: Vec<_> = index.fighters.iter().map(|(name, f)| (name.clone(), f.scripts.len())).collect();
        fighters.sort();
        println!("{} scripts, {} conflicts", index.script_count(), index.conflict_count());
        for (name, count) in fighters {
            println!("  {name:?}: {count}");
        }
        for conflict in &index.conflicts {
            println!("  conflict {}/{}", conflict.fighter, conflict.script);
        }
    }

    const TILTS: &str = r#"use super::*;

unsafe extern "C" fn game_attack_s3_kick(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::ATTACK(agent, 0, 0, Hash40::new("footl"), 8.0, 361, 100, 0, 40, 3.1, 1.0, 0.0, 0.0, None, None, None, 1.0, 1.0, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_M, *COLLISION_SOUND_ATTR_KICK, *ATTACK_REGION_KICK);
    }
}

unsafe extern "C" fn sound_attack_s3_kick(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 5.0);
    if macros::is_excute(agent) {
        macros::PLAY_SE(agent, cmn!("punch_kick_swing_m"));
        macros::PLAY_SE(agent, se!("special_n01"));
    }
}

pub(super) fn install(agent: &mut Agent) {
    agent
        .game_acmd("game_attacks3hop", game_attack_s3_kick, Priority::Default)
        .sound_acmd("sound_attacks3hop", sound_attack_s3_kick, Priority::Default);
}
"#;

    /// Shy Guy's layout: one installer creates the fighter AND an article agent, hands each to a
    /// submodule, keeps its sound macros in `common/`, and has an old copy of the source beside
    /// the crate.
    fn shy_guy_project() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(root, "plugin/Cargo.toml", "[package]\nname = \"shyguy\"\n");
        put(root, "plugin/src/lib.rs", "mod common;\nmod shyguy;\n");
        put(
            root,
            "plugin/src/common/mod.rs",
            "pub mod util;\n// Agent::new(\"fighter\") is how a common agent would look\n",
        );
        put(
            root,
            "plugin/src/common/util.rs",
            r#"#[macro_export]
macro_rules! fighter_se {
    ($fighter:literal, $tone:literal) => {
        smash::phx::Hash40::new(concat!("se_", $fighter, "_", $tone))
    };
}
#[macro_export]
macro_rules! cmn {
    ($tone:literal) => { smash::phx::Hash40::new(concat!("se_common_", $tone)) };
}
"#,
        );
        put(
            root,
            "plugin/src/shyguy/mod.rs",
            r#"macro_rules! se {
    ($tone:literal) => { $crate::fighter_se!("pacman", $tone) };
}
mod articles;
mod pacman;

pub(crate) fn install() {
    let pacman = &mut Agent::new("pacman");
    pacman::install(pacman);
    pacman.install();

    let trampoline = &mut Agent::new("pacman_trampoline");
    articles::trampoline::install(trampoline);
    trampoline.install();
}
"#,
        );
        put(
            root,
            "plugin/src/shyguy/pacman/mod.rs",
            "mod acmd;\npub(super) fn install(agent: &mut Agent) { acmd::install(agent); }\n",
        );
        put(
            root,
            "plugin/src/shyguy/pacman/acmd/mod.rs",
            "mod tilts;\npub(super) fn install(agent: &mut Agent) { tilts::install(agent); }\n",
        );
        put(root, "plugin/src/shyguy/pacman/acmd/tilts.rs", TILTS);
        put(root, "plugin/src/shyguy/articles/mod.rs", "pub mod trampoline;\n");
        put(
            root,
            "plugin/src/shyguy/articles/trampoline/mod.rs",
            r#"unsafe extern "C" fn game_shoot(agent: &mut L2CAgentBase) {}
pub fn install(agent: &mut Agent) { agent.game_acmd("game_shoot", game_shoot, Priority::Low); }
"#,
        );
        // Never compiled: no `mod` reaches it.
        put(root, "plugin/srcOLD/shyguy/pacman/acmd/tilts.rs", TILTS);
        tmp
    }

    #[test]
    fn a_multi_agent_installer_attributes_each_submodule_to_the_agent_it_is_handed() {
        let tmp = shy_guy_project();
        let index = SourceIndex::build(tmp.path()).unwrap();
        assert!(index.conflicts.is_empty(), "{:?}", index.conflicts);
        let mut fighters: Vec<_> = index.fighters.keys().cloned().collect();
        fighters.sort();
        assert_eq!(fighters, ["pacman", "pacman_trampoline"], "no `fighter` from a comment");
        let game = index.script("pacman", "game_attacks3hop").expect("registered game script");
        assert!(game.file.ends_with("plugin/src/shyguy/pacman/acmd/tilts.rs"), "{game:?}");
        // `.sound_acmd(` registrations count, under the registered name.
        assert!(index.script("pacman", "sound_attacks3hop").is_some());
        assert!(index.script("pacman", "sound_attack_s3_kick").is_none());
        assert!(index.script("pacman_trampoline", "game_shoot").is_some());
        assert!(
            !index.files.iter().any(|f| f.to_string_lossy().contains("srcOLD")),
            "a backup copy outside the crate is not indexed"
        );
    }

    #[test]
    fn project_sound_macros_read_as_names_and_edits_keep_the_macro() {
        let tmp = shy_guy_project();
        let index = SourceIndex::build(tmp.path()).unwrap();
        let project = index.script_source("pacman", "attacks3hop").unwrap();
        assert!(
            project.body.contains(r#"Hash40::new("se_common_punch_kick_swing_m")"#),
            "{}",
            project.body
        );
        let events = crate::acmd::parse_sound_script(&project.body).to_sound_events();
        assert_eq!(events.len(), 2, "both macro-written sounds are typed");
        assert_eq!(events[1].call.sounds, ["se_pacman_special_n01"]);

        let mut edited = events.clone();
        edited[1].call.sounds[0] = "se_pacman_special_n02".into();
        edited[0].call.sounds[0] = "se_common_swing_01".into();
        let file = tmp.path().join("plugin/src/shyguy/pacman/acmd/tilts.rs");
        let site = index.script("pacman", "sound_attacks3hop").unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        let body = &text[site.span.clone()];
        let (after, report) =
            rewrite_sounds_with(body, "t", &events, &edited, &index.hash_macros).unwrap();
        assert_eq!(report.changed, 2, "{:?}", report.skipped);
        assert!(after.contains(r#"macros::PLAY_SE(agent, se!("special_n02"));"#), "{after}");
        assert!(after.contains(r#"macros::PLAY_SE(agent, cmn!("swing_01"));"#), "{after}");
        assert!(!after.contains("Hash40::new(\"se_"), "no line was restyled: {after}");
    }

    /// A project that glob-imports smash_script's macros calls them bare. Reads see the calls,
    /// and a value edit changes the value — the call stays bare.
    #[test]
    fn bare_macro_calls_are_read_and_edited_without_gaining_a_path() {
        let source = r#"unsafe extern "C" fn game_attack11(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 3.0);
    if is_excute(agent) {
        ATTACK(agent, 0, 0, Hash40::new("top"), 3.0, 361, 100, 0, 20, 3.0, 0.0, 8.0, 6.0, None, None, None, 1.0, 1.0, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_PUNCH, *ATTACK_REGION_PUNCH);
    }
    wait(agent.lua_state_agent, 2.0);
    if is_excute(agent) {
        AttackModule::clear_all(agent.module_accessor);
    }
}
"#;
        let pristine = crate::acmd::parse_acmd_script(source).to_hitboxes();
        assert_eq!(pristine.len(), 1, "the bare ATTACK is read");
        assert_eq!(pristine[0].active_start, 3);
        let mut edited = pristine.clone();
        edited[0].damage = 4.5;
        let (after, report) = rewrite_hitboxes(source, "t", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{:?}", report.skipped);
        assert!(
            after.contains("        ATTACK(agent, 0, 0, Hash40::new(\"top\"), 4.5, 361"),
            "{after}"
        );
        assert!(!after.contains("macros::"), "{after}");
        assert_eq!(after.lines().count(), source.lines().count());
    }

    /// Bluster Kong registers `game_landingairhi` to `game_attackairhi` and leaves an unused
    /// `fn game_landingairhi` behind; Sackboy registers `effect_fall` twice. Iron Man has an
    /// ACMD `stub` and a status `stub` in different files.
    #[test]
    fn the_registered_function_is_the_script_not_a_namesake() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(
            root,
            "src/acmd.rs",
            r#"unsafe extern "C" fn game_attackairhi(agent: &mut L2CAgentBase) {}
unsafe extern "C" fn game_landingairhi(agent: &mut L2CAgentBase) {}
unsafe extern "C" fn effect_fall(agent: &mut L2CAgentBase) {}
unsafe extern "C" fn effect_burst(agent: &mut L2CAgentBase) {}
unsafe extern "C" fn stub(agent: &mut L2CAgentBase) {}
pub fn install() {
    let agent = &mut Agent::new("donkey");
    agent.game_acmd("game_attackairhi", game_attackairhi, Priority::Default);
    agent.game_acmd("game_landingairhi", game_attackairhi, Priority::Default);
    agent.acmd("effect_fall", effect_fall, Priority::Default);
    agent.acmd("effect_fall", effect_burst, Priority::Default);
    agent.acmd("effect_appeallwr", stub, Priority::Default);
}
"#,
        );
        put(
            root,
            "src/status.rs",
            "unsafe extern \"C\" fn stub(fighter: &mut L2CFighterCommon) -> L2CValue { 0.into() }\n",
        );
        let index = SourceIndex::build(root).unwrap();
        assert!(index.conflicts.is_empty(), "{:?}", index.conflicts);
        assert_eq!(
            index.script("donkey", "game_landingairhi").unwrap().function,
            "game_attackairhi"
        );
        assert_eq!(index.script("donkey", "effect_fall").unwrap().function, "effect_burst");
        assert!(index.script("donkey", "effect_appeallwr").unwrap().file.ends_with("acmd.rs"));
    }

    /// An inserted line takes the file's own spelling. In a project that glob-imports the
    /// macros, names its agent `fighter`, indents with tabs and checks out with CRLF, a
    /// canonical `macros::REVERSE_LR(agent);` would not compile and would mix line endings.
    #[test]
    fn inserted_lines_match_the_file_they_land_in() {
        let mut text = "unsafe extern \"C\" fn game_x(fighter: &mut L2CAgentBase) {\r\n\tframe(fighter.lua_state_agent, 3);\r\n\tif is_excute(fighter) {\r\n\t\tATTACK(fighter, 0);\r\n\t}\r\n}\r\n".to_string();
        assert!(insert_reverse_line(&mut text, 3));
        assert!(insert_reverse_line(&mut text, 7));
        assert!(text.contains("\t\tATTACK(fighter, 0);\r\n\t\tREVERSE_LR(fighter);\r\n\t}"), "{text:?}");
        assert!(
            text.contains("\tframe(fighter.lua_state_agent, 7);\r\n\tif is_excute(fighter) {\r\n\t\tREVERSE_LR(fighter);\r\n\t}\r\n"),
            "{text:?}"
        );
        assert!(!text.contains("macros::") && !text.contains("agent)"), "{text:?}");
        assert_eq!(text.matches('\n').count(), text.matches("\r\n").count(), "no bare LF");
        // And the line it wrote is recognised as the one to take back out.
        let line = text.lines().find(|l| l.contains("REVERSE_LR")).unwrap().trim();
        assert!(is_lone_agent_call(line, "REVERSE_LR"));
    }

    /// Amy, Captain America and others name the agent `fighter`. Its calls are the agent's own
    /// and are typed; a call on some other object stays raw, as it always did.
    #[test]
    fn a_fighter_named_agent_is_still_the_agent() {
        let source = "unsafe extern \"C\" fn game_x(fighter: &mut L2CAgentBase) {\n    frame(fighter.lua_state_agent, 2.0);\n    macros::FT_MOTION_RATE(fighter, 0.5);\n    frame(fighter.lua_state_agent, 6.0);\n    if macros::is_excute(fighter) {\n        macros::SET_AIR(fighter);\n        macros::SET_AIR(other);\n    }\n}\n";
        let script = crate::acmd::parse_acmd_script(source);
        assert!(
            script.stmts.iter().any(|s| matches!(s, crate::data::AcmdStmt::MotionRate(r) if *r == 0.5)),
            "{:?}",
            script.stmts
        );
        assert_eq!(script.to_set_air_events().len(), 1, "only the fighter's own SET_AIR");
        // Outside any scope, the name means nothing special again.
        assert!(!crate::acmd::is_agent_arg("fighter"));
    }

    /// A hitbox tuned through named constants — Shy Guy's walk kick keeps its numbers in
    /// `const`s — reads as a hitbox, and an edit changes the constant where it is defined.
    #[test]
    fn a_value_written_as_a_const_is_edited_at_its_definition() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(root, "Cargo.toml", "[package]\nname = \"m\"\n");
        put(root, "src/lib.rs", "mod mario;\n");
        put(
            root,
            "src/mario/mod.rs",
            "mod acmd;\nmod params;\npub fn install() { let agent = &mut Agent::new(\"mario\"); acmd::install(agent); }\n",
        );
        put(root, "src/mario/params.rs", "pub const JAB_DAMAGE:   f32 = 3.0;\npub const JAB_ID: i32 = 0;\n");
        let acmd = "use super::params::*;\nunsafe extern \"C\" fn game_attack11(agent: &mut L2CAgentBase) {\n    frame(agent.lua_state_agent, 2.0);\n    if macros::is_excute(agent) {\n        macros::ATTACK(agent, JAB_ID as u64, 0, Hash40::new(\"top\"), JAB_DAMAGE, 361, 100, 0, 20, 3.0, 0.0, 8.0, 6.0, None, None, None, 1.0, 1.0, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new(\"collision_attr_normal\"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_PUNCH, *ATTACK_REGION_PUNCH);\n    }\n}\npub fn install(agent: &mut Agent) { agent.game_acmd(\"game_attack11\", game_attack11, Priority::Default); }\n";
        put(root, "src/mario/acmd.rs", acmd);
        let index = SourceIndex::build(root).unwrap();
        let body = index.script_source("mario", "attack11").unwrap().body;
        let pristine = crate::acmd::parse_acmd_script(&body).to_hitboxes();
        assert_eq!(pristine.len(), 1, "{body}");
        assert_eq!(pristine[0].damage, 3.0);
        let mut edited = pristine.clone();
        edited[0].damage = 4.5;
        let report = sync_hitboxes(&index, "mario", "attack11", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{:?}", report.skipped);
        assert_eq!(
            std::fs::read_to_string(root.join("src/mario/params.rs")).unwrap(),
            "pub const JAB_DAMAGE:   f32 = 4.5;\npub const JAB_ID: i32 = 0;\n"
        );
        assert_eq!(std::fs::read_to_string(root.join("src/mario/acmd.rs")).unwrap(), acmd, "the call keeps its name");
        assert!(report.skipped.iter().any(|note| note.contains("JAB_DAMAGE is a constant")), "{:?}", report.skipped);
    }

    /// Shy Guy's `RUMBLE_HIT(agent, kind)` helper stands for `RUMBLE_HIT(agent, kind, 0)`. The
    /// helper call reads as that rumble, a vanilla-shaped call beside it keeps its own number,
    /// an edited kind keeps the author's `rb!`, and the helper's fixed `0` is not writable here.
    #[test]
    fn a_project_helper_call_is_read_and_edited_through_its_target() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(root, "Cargo.toml", "[package]\nname = \"m\"\n");
        put(root, "src/lib.rs", "mod common;\nmod mario;\n");
        put(
            root,
            "src/common.rs",
            "pub unsafe fn RUMBLE_HIT(agent: &mut L2CAgentBase, kind: Hash40) {\n    smash_script::macros::RUMBLE_HIT(agent, kind, 0);\n}\nmacro_rules! rb {\n    ($kind:literal) => { smash::phx::Hash40::new(concat!(\"rbkind_\", $kind)) };\n}\n",
        );
        put(root, "src/mario/mod.rs", "mod acmd;\npub fn install() { let agent = &mut Agent::new(\"mario\"); acmd::install(agent); }\n");
        let acmd = "unsafe extern \"C\" fn expression_attack11(agent: &mut L2CAgentBase) {\n    frame(agent.lua_state_agent, 2.0);\n    if macros::is_excute(agent) {\n        macros::RUMBLE_HIT(agent, rb!(\"attackm\"));\n        macros::RUMBLE_HIT(agent, Hash40::new(\"rbkind_attacks\"), 0);\n    }\n}\npub fn install(agent: &mut Agent) { agent.expression_acmd(\"expression_attack11\", expression_attack11, Priority::Default); }\n";
        put(root, "src/mario/acmd.rs", acmd);
        let index = SourceIndex::build(root).unwrap();
        let body = index.script_source("mario", "attack11").unwrap().body;
        let events = crate::acmd::parse_expression_script(&body).to_expression_events();
        assert_eq!(events.len(), 2, "{body}");
        assert_eq!(
            events[0].call,
            crate::data::ExpressionCall::RumbleHit {
                kind: "Hash40::new(\"rbkind_attackm\")".into(),
                unk: "0".into()
            }
        );

        // Retune both kinds and the helper's hidden argument.
        let mut edited = events.clone();
        edited[0].call = crate::data::ExpressionCall::RumbleHit {
            kind: "Hash40::new(\"rbkind_attackl\")".into(),
            unk: "4".into(),
        };
        edited[1].call = crate::data::ExpressionCall::RumbleHit {
            kind: "Hash40::new(\"rbkind_attackll\")".into(),
            unk: "0".into(),
        };
        let report = sync_expressions(&index, "mario", "attack11", &events, &edited).unwrap();
        let written = std::fs::read_to_string(root.join("src/mario/acmd.rs")).unwrap();
        assert!(written.contains("        macros::RUMBLE_HIT(agent, rb!(\"attackl\"));\n"), "{written}");
        assert!(
            written.contains("        macros::RUMBLE_HIT(agent, Hash40::new(\"rbkind_attackll\"), 0);\n"),
            "the vanilla-shaped call is the second site, not the first: {written}"
        );
        assert!(
            report.skipped.iter().any(|note| note.contains("helper")),
            "the fixed argument is refused with a reason: {:?}",
            report.skipped
        );
    }

    #[test]
    fn a_block_written_on_one_line_parses_instead_of_panicking() {
        let source = r#"unsafe extern "C" fn game_x(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) { macros::ATTACK(agent, 0, 0, Hash40::new("top"), 3.0, 361, 100, 0, 20, 3.0, 0.0, 8.0, 6.0, None, None, None, 1.0, 1.0, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new("collision_attr_normal"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_PUNCH, *ATTACK_REGION_PUNCH); }
    if foo(&Vector2f{x: 1.0, y: 0.0}) {
        bar();
    }
    frame(agent.lua_state_agent, 9.0);
}
"#;
        let script = crate::acmd::parse_acmd_script(source);
        assert_eq!(script.to_hitboxes().len(), 1);
        assert_eq!(script.to_hitboxes()[0].active_start, 4);
        let effect = source.replace("game_x", "effect_x");
        let _ = crate::acmd::parse_effect_script(&effect);
    }
}

#[cfg(test)]
mod rewrite_article_tests {
    use super::*;
    use crate::data::{ArticleCall, ArticleTarget};

    /// A charge-shot spawn and its shoot, each in its own frame block, in the shape the vanilla
    /// dump and the corpus write them (`samus/game_specialn`).
    const SPECIAL_N: &str = "unsafe extern \"C\" fn game_specialn(agent: &mut L2CAgentBase) {\n\
        \x20   frame(agent.lua_state_agent, 10.0);\n\
        \x20   if macros::is_excute(agent) {\n\
        \x20       ArticleModule::generate_article(agent.module_accessor, *FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT, false, -1);\n\
        \x20   }\n\
        \x20   frame(agent.lua_state_agent, 20.0);\n\
        \x20   if macros::is_excute(agent) {\n\
        \x20       ArticleModule::shoot(agent.module_accessor, *FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT, ArticleOperationTarget(*ARTICLE_OPE_TARGET_LAST), false);\n\
        \x20   }\n\
        }\n";

    fn calls(text: &str) -> Vec<(u32, ArticleCall)> {
        crate::acmd::parse_acmd_script(text).to_article_calls()
    }

    fn rewrite(text: &str, edit: impl FnOnce(&mut Vec<(u32, ArticleCall)>)) -> (String, SyncReport) {
        let pristine = calls(text);
        let mut edited = pristine.clone();
        edit(&mut edited);
        rewrite_article_calls(text, "samus/specialn", &pristine, &edited).unwrap()
    }

    #[test]
    fn edits_target_only() {
        let (out, report) = rewrite(SPECIAL_N, |calls| {
            assert_eq!(calls.len(), 2, "{calls:#?}");
            calls[1].1.target = ArticleTarget::All;
            calls[1].1.edited = true;
        });
        assert_eq!(out, SPECIAL_N.replace("TARGET_LAST", "TARGET_ALL"), "{report:?}");
        assert_eq!(report.changed, 1);
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(calls(&out)[1].1.target, ArticleTarget::All);
    }

    /// A bare `*ARTICLE_OPE_TARGET_X` is the timeline's canonical form wrapped; the file keeps
    /// its own bare spelling, and compact commas stay compact, because only the target's bytes
    /// are replaced.
    #[test]
    fn edits_keep_the_file_spelling() {
        let text = SPECIAL_N.replace(
            "ArticleModule::shoot(agent.module_accessor, *FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT, ArticleOperationTarget(*ARTICLE_OPE_TARGET_LAST), false);",
            "ArticleModule::shoot(agent.module_accessor,*FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT,*ARTICLE_OPE_TARGET_LAST,false);",
        );
        let (out, report) = rewrite(&text, |calls| {
            calls[1].1.target = ArticleTarget::All;
            calls[1].1.edited = true;
        });
        assert_eq!(out, text.replace("TARGET_LAST", "TARGET_ALL"), "{report:?}");
        // The receiver path, the binding and an argument edit.
        let text = SPECIAL_N.replace(
            "ArticleModule::generate_article(agent.module_accessor, *FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT, false, -1);",
            "let id = smash::app::lua_bind::ArticleModule::generate_article(boma,  *FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT, false, -1);",
        );
        let (out, report) = rewrite(&text, |calls| {
            calls[0].1.args[0] = "true".into();
            calls[0].1.edited = true;
        });
        assert_eq!(out, text.replace("CSHOT, false, -1", "CSHOT, true, -1"), "{report:?}");
    }

    /// The only call in its frame block: the block is retimed and placed in frame order, the way
    /// effect blocks move.
    #[test]
    fn moves_spawn_to_new_frame() {
        let (out, report) = rewrite(SPECIAL_N, |calls| calls[0].0 = 14);
        assert_eq!(out, SPECIAL_N.replace("10.0", "14.0"), "{report:?}");
        assert!(out.contains("frame(agent.lua_state_agent, 14.0);\n    if macros::is_excute(agent) {\n        ArticleModule::generate_article("));
        assert_eq!(calls(&out)[0].0, 14);
        assert_eq!(report.changed, 1);

        // Past the shoot: the block lands after frame 20, so the calls swap order.
        let (out, report) = rewrite(SPECIAL_N, |calls| calls[0].0 = 25);
        let moved = calls(&out);
        assert_eq!((moved[0].0, moved[1].0), (20, 25), "{out}\n{report:?}");
        assert_eq!(moved[1].1.op, crate::data::ArticleOp::Generate);
        assert_eq!(out.len(), SPECIAL_N.len());
    }

    /// A call sharing its block moves alone: into the target frame's block when there is one,
    /// else into a new block built like the one it left.
    #[test]
    fn moves_one_line_between_blocks() {
        let shared = SPECIAL_N.replace(
            "        ArticleModule::generate_article(agent.module_accessor, *FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT, false, -1);\n",
            "        ArticleModule::generate_article(agent.module_accessor, *FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT, false, -1);\n        macros::ATTACK(agent, 0, 0, Hash40::new(\"top\"), 1.0, 361, 100, 0, 0, 3.0, 0.0, 8.0, 8.0, None, None, None, 1.0, 1.0, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_F, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new(\"collision_attr_normal\"), *ATTACK_SOUND_LEVEL_S, *COLLISION_SOUND_ATTR_PUNCH, *ATTACK_REGION_NONE);\n",
        );
        // Into frame 20's existing block, after its own call.
        let (out, report) = rewrite(&shared, |calls| calls[0].0 = 20);
        let moved = calls(&out);
        assert_eq!((moved[0].0, moved[1].0), (20, 20), "{out}\n{report:?}");
        assert_eq!(moved[1].1.op, crate::data::ArticleOp::Generate);
        assert!(out.contains("frame(agent.lua_state_agent, 10.0);\n    if macros::is_excute(agent) {\n        macros::ATTACK("), "{out}");
        // Into a new frame 14 block.
        let (out, report) = rewrite(&shared, |calls| calls[0].0 = 14);
        let moved = calls(&out);
        assert_eq!((moved[0].0, moved[1].0), (14, 20), "{out}\n{report:?}");
        assert!(out.contains("    }\n    frame(agent.lua_state_agent, 14.0);\n    if macros::is_excute(agent) {\n        ArticleModule::generate_article(agent.module_accessor, *FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT, false, -1);\n    }\n    frame(agent.lua_state_agent, 20.0);"), "{out}");
        assert!(out.contains("10.0);\n    if macros::is_excute(agent) {\n        macros::ATTACK("), "{out}");
        assert_eq!(crate::acmd::parse_acmd_script(&out).to_hitboxes().len(), 1);
        // Earlier than the block it leaves: the new block goes before that block's frame line.
        let (out, report) = rewrite(&shared, |calls| calls[0].0 = 5);
        let moved = calls(&out);
        assert_eq!((moved[0].0, moved[1].0), (5, 20), "{out}\n{report:?}");
        let (five, ten) = (out.find("5.0);").unwrap(), out.find("10.0);").unwrap());
        assert!(five < ten, "{out}");
    }

    /// Comments, a `let` binding and an `if` condition around the calls: the statements are
    /// counted as the parser types them.
    #[test]
    fn statement_sites_skip_comments_and_conditions() {
        let text = SPECIAL_N
            .replace(
                "        ArticleModule::generate_article(",
                "        // spawn the shot\n        /* charged */ \n        ArticleModule::generate_article(",
            )
            .replace(
                "    frame(agent.lua_state_agent, 20.0);\n",
                "    frame(agent.lua_state_agent, 20.0); // fire\n",
            );
        let sites = article_call_sites(&text);
        assert_eq!(sites.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["generate_article", "shoot"]);
        let condition = "    if ArticleModule::is_exist(agent.module_accessor, *FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT) {\n    }\n";
        let with_condition = format!("{}{condition}}}\n", &SPECIAL_N[..SPECIAL_N.len() - 2]);
        assert_eq!(article_call_sites(&with_condition).len(), 2, "{with_condition}");
        assert_eq!(calls(&with_condition).len(), 2);
        let (out, report) = rewrite(&text, |calls| calls[0].0 = 14);
        assert_eq!(out, text.replace("10.0", "14.0"), "{report:?}");
    }

    #[test]
    fn count_mismatch_skips_with_reason() {
        let pristine = calls(SPECIAL_N);
        let mut edited = pristine.clone();
        edited[1].1.target = ArticleTarget::All;
        // A third call the editor does not model by position (inside a condition).
        let text = SPECIAL_N.replace(
            "    frame(agent.lua_state_agent, 20.0);\n",
            "    if WorkModule::is_flag(agent.module_accessor, *FIGHTER_SAMUS_STATUS_SPECIAL_N_FLAG_CHARGE) {\n        ArticleModule::remove_exist(agent.module_accessor, *FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT, ArticleOperationTarget(*ARTICLE_OPE_TARGET_ALL));\n    }\n    frame(agent.lua_state_agent, 20.0);\n",
        );
        let (out, report) = rewrite_article_calls(&text, "samus/specialn", &pristine, &edited).unwrap();
        assert_eq!(out, text);
        assert_eq!(report.changed, 0);
        assert_eq!(report.skipped.len(), 1, "{report:?}");
        assert!(report.skipped[0].contains("3 `ArticleModule` call(s)"), "{report:?}");
        assert!(report.skipped[0].contains("models 2"), "{report:?}");

        // Added or removed calls are structural too.
        let (out, report) = rewrite_article_calls(SPECIAL_N, "samus/specialn", &pristine, &edited[..1]).unwrap();
        assert_eq!(out, SPECIAL_N);
        assert!(report.skipped[0].contains("added or removed"), "{report:?}");

        // Nothing edited: nothing reported, even where the counts disagree.
        let (out, report) = rewrite_article_calls(&text, "samus/specialn", &pristine, &pristine).unwrap();
        assert_eq!((out.as_str(), report), (text.as_str(), SyncReport::default()));
    }

    /// Paired by position, but the call at that position is a different one: refused.
    #[test]
    fn mispaired_calls_are_refused() {
        let pristine = calls(SPECIAL_N);
        let mut other = pristine.clone();
        other[0].1.article = "*FIGHTER_SAMUS_GENERATE_ARTICLE_SUPERMISSILE".into();
        let mut edited = other.clone();
        edited[1].1.target = ArticleTarget::All;
        let (out, report) = rewrite_article_calls(SPECIAL_N, "samus/specialn", &other, &edited).unwrap();
        assert_eq!(out, SPECIAL_N);
        assert!(report.skipped[0].contains("is not what the source holds there"), "{report:?}");
        assert!(report.skipped[0].contains("SUPERMISSILE"), "{report:?}");
        // An argument that differs is a different call too, not only the function or article.
        let mut other = pristine.clone();
        other[0].1.args[0] = "true".into();
        let (out, report) = rewrite_article_calls(SPECIAL_N, "samus/specialn", &other, &other).unwrap();
        assert_eq!(out, SPECIAL_N);
        assert!(report.skipped.is_empty(), "nothing edited: {report:?}");
        let mut edited = other.clone();
        edited[0].1.args[1] = "0".into();
        let (out, report) = rewrite_article_calls(SPECIAL_N, "samus/specialn", &other, &edited).unwrap();
        assert_eq!(out, SPECIAL_N);
        assert!(report.skipped[0].contains("passes `false` where the list has `true`"), "{report:?}");
        // The editor's list in another order than the file's pairs each call with its own.
        let swapped = vec![pristine[1].clone(), pristine[0].clone()];
        let mut edited = swapped.clone();
        edited[0].1.target = ArticleTarget::All;
        let (out, report) = rewrite_editor_article_calls(SPECIAL_N, "samus/specialn", &swapped, &edited).unwrap();
        assert_eq!(out, SPECIAL_N.replace("TARGET_LAST", "TARGET_ALL"), "{report:?}");
    }

    /// One editor session, synced after every edit: `pristine` stays the list loaded at the start,
    /// as the editor keeps it; the baseline is the file each time.
    fn sync_each(text: &str, pristine: &[(u32, ArticleCall)], edited: &[(u32, ArticleCall)]) -> String {
        let (out, report) = rewrite_editor_article_calls(text, "samus/specialn", pristine, edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}\n{out}");
        out
    }

    /// I1: a written move carries a call past another; the next sync still pairs and writes.
    #[test]
    fn second_sync_after_a_carry_past_move() {
        let pristine = calls(SPECIAL_N);
        let mut edited = pristine.clone();
        edited[0].0 = 25;
        let first = sync_each(SPECIAL_N, &pristine, &edited);
        assert_eq!(sync_each(&first, &pristine, &edited), first, "re-running writes nothing");
        edited[1].1.target = ArticleTarget::All;
        edited[0].1.args[0] = "true".into();
        let second = sync_each(&first, &pristine, &edited);
        assert_eq!(
            second,
            first.replace("TARGET_LAST", "TARGET_ALL").replace("CSHOT, false, -1", "CSHOT, true, -1")
        );
    }

    /// Important B: the same call retimed twice, 10→25 then 25→30, without a reload.
    #[test]
    fn a_call_retimed_twice() {
        let pristine = calls(SPECIAL_N);
        let mut edited = pristine.clone();
        edited[0].0 = 25;
        let first = sync_each(SPECIAL_N, &pristine, &edited);
        edited[0].0 = 30;
        let second = sync_each(&first, &pristine, &edited);
        let now = calls(&second);
        assert_eq!((now[0].0, now[1].0), (20, 30), "{second}");
        assert_eq!(second.len(), SPECIAL_N.len(), "only the block moved and its frame changed: {second}");
        assert!(second.ends_with("    frame(agent.lua_state_agent, 30.0);
    if macros::is_excute(agent) {
        ArticleModule::generate_article(agent.module_accessor, *FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT, false, -1);
    }
}
"), "{second}");
    }

    /// Important B: the same call's article changed twice, A→B then B→C, without a reload.
    #[test]
    fn an_article_changed_twice() {
        let pristine = calls(SPECIAL_N);
        let mut edited = pristine.clone();
        edited[0].1.article = "*FIGHTER_SAMUS_GENERATE_ARTICLE_SUPERMISSILE".into();
        let first = sync_each(SPECIAL_N, &pristine, &edited);
        assert_eq!(first, SPECIAL_N.replacen("ARTICLE_CSHOT, false", "ARTICLE_SUPERMISSILE, false", 1));
        edited[0].1.article = "*FIGHTER_SAMUS_GENERATE_ARTICLE_MISSILE".into();
        edited[0].1.args[1] = "0".into();
        let second = sync_each(&first, &pristine, &edited);
        assert_eq!(second, SPECIAL_N.replacen("ARTICLE_CSHOT, false, -1", "ARTICLE_MISSILE, false, 0", 1));
    }

    /// Important A: two `generate_article` calls of the same article with different arguments.
    /// The first moves onto the second's frame and lands after it, so the file lists them the other
    /// way round; a later edit to the first changes only the first's bytes.
    #[test]
    fn same_article_calls_keep_their_own_edits() {
        let text = SPECIAL_N.replace(
            "ArticleModule::shoot(agent.module_accessor, *FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT, ArticleOperationTarget(*ARTICLE_OPE_TARGET_LAST), false);",
            "ArticleModule::generate_article(agent.module_accessor, *FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT, true, 7);",
        );
        let pristine = calls(&text);
        assert_eq!(pristine.len(), 2);
        let mut edited = pristine.clone();
        edited[0].0 = 20;
        let first = sync_each(&text, &pristine, &edited);
        let lines: Vec<&str> = first.lines().filter(|line| line.contains("generate_article")).collect();
        assert!(lines[0].contains("true, 7") && lines[1].contains("false, -1"), "{first}");
        edited[0].1.args[1] = "3".into();
        let second = sync_each(&first, &pristine, &edited);
        assert_eq!(second, first.replace("CSHOT, false, -1", "CSHOT, false, 3"), "{second}");
        assert!(second.contains("CSHOT, true, 7"), "the other call is untouched: {second}");
    }

    /// Cuphead-Moveset · src/buddy/acmd/specials.rs:78-96 (`game_specialn2`, tier 1, smashline2)
    /// spawns `*FIGHTER_BUDDY_GENERATE_ARTICLE_BULLET` four times at frame 14 with the same
    /// arguments. While the file still holds the calls as loaded, an edit to one of them writes
    /// that one, and putting it back restores every byte.
    #[test]
    fn identical_calls_as_loaded_pair_by_position() {
        let line = "ArticleModule::generate_article(agent.module_accessor, *FIGHTER_BUDDY_GENERATE_ARTICLE_BULLET, false, -1);";
        let text = format!(
            "unsafe extern \"C\" fn game_specialn2(agent: &mut L2CAgentBase) {{\r\n    frame(agent.lua_state_agent, 14.0);\r\n    if macros::is_excute(agent) {{\r\n            {line}\r\n            {line}\r\n    }}\r\n}}\r\n"
        );
        let pristine = calls(&text);
        assert_eq!(pristine.len(), 2);
        let mut edited = pristine.clone();
        edited[1].1.args[1] = "0".into();
        let (out, report) = rewrite_editor_article_calls(&text, "buddy/specialn2", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        let second = text.rfind("false, -1").unwrap();
        assert_eq!(out, format!("{}false, 0{}", &text[..second], &text[second + "false, -1".len()..]));
        let (back, report) = rewrite_editor_article_calls(&out, "buddy/specialn2", &edited, &pristine).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(back, text);
    }

    /// Calls the mapping cannot tell apart are refused with the reason, and nothing is written.
    #[test]
    fn ambiguous_mapping_is_refused() {
        let line = "ArticleModule::generate_article(agent.module_accessor, *FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT, false, -1);";
        let call = ArticleCall::parse(line).unwrap();
        // The editor retimed both identical calls; the file holds them at frames neither had.
        let pristine = vec![(10, call.clone()), (20, call.clone())];
        let edited = vec![(30, call.clone()), (40, call.clone())];
        let source = vec![(25, call.clone()), (35, call.clone())];
        let error = article_calls_in_source_order(&source, &pristine, &edited).unwrap_err();
        assert!(error.contains("could be more than one of the editor's article calls"), "{error}");
        assert!(!error.contains("  "), "one sentence, no stray spacing: {error}");
        // Told apart by frame, the same lists pair.
        let source = vec![(20, call.clone()), (30, call.clone())];
        let ordered = article_calls_in_source_order(&source, &pristine, &edited).unwrap();
        assert_eq!((ordered[0].0, ordered[1].0), (40, 30));
        // Identical calls edited identically are interchangeable.
        let same = vec![(10, call.clone()), (10, call.clone())];
        assert!(article_calls_in_source_order(&same, &same, &same).is_ok());
        // Through the writer: the reason is reported and the text is unchanged.
        let text = format!(
            "unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {{\n    frame(agent.lua_state_agent, 25.0);\n    if macros::is_excute(agent) {{\n        {line}\n    }}\n    frame(agent.lua_state_agent, 35.0);\n    if macros::is_excute(agent) {{\n        {line}\n    }}\n}}\n"
        );
        let (out, report) = rewrite_editor_article_calls(&text, "samus/x", &pristine, &edited).unwrap();
        assert_eq!(out, text);
        assert!(report.skipped[0].contains("could be more than one"), "{report:?}");
        // A different number of calls is refused too.
        let error = article_calls_in_source_order(&source[..1], &pristine, &edited).unwrap_err();
        assert!(error.contains("1 article call(s) where the editor has 2"), "{error}");
    }

    /// N1: an argument edited twice without a reload. The file holds a value from the first edit
    /// that neither the call's original nor its new edit has; it must not be taken for the other
    /// call that happens to hold that value. Refused, with nothing written.
    #[test]
    fn an_in_between_value_is_not_taken_for_another_call() {
        let text = SPECIAL_N.replace(
            "ArticleModule::shoot(agent.module_accessor, *FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT, ArticleOperationTarget(*ARTICLE_OPE_TARGET_LAST), false);",
            "ArticleModule::generate_article(agent.module_accessor, *FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT, true, 5);",
        );
        let pristine = calls(&text);
        let mut edited = pristine.clone();
        edited[0].1.args = vec!["true".into(), "5".into()];
        let first = sync_each(&text, &pristine, &edited);
        assert_eq!(first, text.replacen("CSHOT, false, -1", "CSHOT, true, 5", 1));
        edited[0].1.args = vec!["false".into(), "3".into()];
        let (second, report) = rewrite_editor_article_calls(&first, "samus/specialn", &pristine, &edited).unwrap();
        assert_eq!(second, first, "nothing is written");
        assert_eq!(report.skipped.len(), 1, "{report:?}");
        assert!(report.skipped[0].contains("matches none of the editor's article calls"), "{report:?}");
    }

    /// N2: an edit that sets only `target` (its argument text still the old spelling) is kept
    /// through later syncs instead of being written back.
    #[test]
    fn a_target_edit_survives_later_syncs() {
        let pristine = calls(SPECIAL_N);
        let mut edited = pristine.clone();
        edited[1].1.target = ArticleTarget::All;
        assert!(edited[1].1.args[0].contains("LAST"), "the argument text is stale on purpose");
        let first = sync_each(SPECIAL_N, &pristine, &edited);
        assert_eq!(first, SPECIAL_N.replace("TARGET_LAST", "TARGET_ALL"));
        for _ in 0..2 {
            let (again, report) = rewrite_editor_article_calls(&first, "samus/specialn", &pristine, &edited).unwrap();
            assert_eq!((again.as_str(), report.changed), (first.as_str(), 0), "{report:?}");
            assert!(report.skipped.is_empty(), "{report:?}");
        }
    }

    /// N3: a synced edit put back to its original in the editor is never dropped silently: the
    /// file returns to the original, or the refusal is reported.
    #[test]
    fn restoring_a_synced_value_is_written_or_reported() {
        let pristine = calls(SPECIAL_N);
        let mut edited = pristine.clone();
        edited[0].1.args[0] = "true".into();
        let first = sync_each(SPECIAL_N, &pristine, &edited);
        assert_ne!(first, SPECIAL_N);
        let (second, report) = rewrite_editor_article_calls(&first, "samus/specialn", &pristine, &pristine).unwrap();
        assert!(second == SPECIAL_N || !report.skipped.is_empty(), "silent: {report:?}\n{second}");
        assert!(second == SPECIAL_N || second == first, "{second}");
        // A file that already holds the edited calls, in any order, has nothing to report.
        let (_, report) = rewrite_editor_article_calls(&first, "samus/specialn", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
    }

    /// I1: a written article change renames the call; a later edit to it still lands.
    #[test]
    fn second_edit_after_an_article_change() {
        let pristine = calls(SPECIAL_N);
        let mut edited = pristine.clone();
        edited[0].1.article = "*FIGHTER_SAMUS_GENERATE_ARTICLE_SUPERMISSILE".into();
        let first = sync_each(SPECIAL_N, &pristine, &edited);
        edited[0].1.args[1] = "0".into();
        let second = sync_each(&first, &pristine, &edited);
        assert_eq!(second, first.replace("SUPERMISSILE, false, -1", "SUPERMISSILE, false, 0"));
    }

    /// The same two syncs through the project on disk, as the editor runs them.
    #[test]
    fn two_syncs_on_disk_after_a_carry_past_move() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("src/samus/acmd.rs");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, SPECIAL_N).unwrap();
        std::fs::write(
            tmp.path().join("src/samus/mod.rs"),
            "pub fn install() { let agent = &mut smashline::Agent::new(\"samus\"); }",
        )
        .unwrap();
        let pristine = calls(SPECIAL_N);
        let mut edited = pristine.clone();
        edited[0].0 = 25;
        let index = SourceIndex::build(tmp.path()).unwrap();
        let report = sync_article_calls(&index, "samus", "special_n", &pristine, &edited).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        edited[1].1.target = ArticleTarget::All;
        let index = SourceIndex::build(tmp.path()).unwrap();
        let report = sync_article_calls(&index, "samus", "special_n", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(report.changed, 1, "{report:?}");
        let on_disk = std::fs::read_to_string(&path).unwrap();
        let moved = calls(&on_disk);
        assert_eq!((moved[0].0, moved[1].0), (20, 25), "{on_disk}");
        assert_eq!(moved[0].1.target, ArticleTarget::All, "{on_disk}");
    }

    #[test]
    fn changing_the_function_is_reported() {
        let (out, report) = rewrite(SPECIAL_N, |calls| {
            calls[1].1.op = crate::data::ArticleOp::ShootExist;
            calls[1].1.edited = true;
        });
        assert_eq!(out, SPECIAL_N);
        assert!(report.skipped[0].contains("`shoot` to `shoot_exist`"), "{report:?}");
    }

    #[test]
    fn crlf_preserved() {
        let text = SPECIAL_N.replace('\n', "\r\n");
        let (out, report) = rewrite(&text, |calls| {
            calls[1].1.target = ArticleTarget::All;
            calls[1].1.edited = true;
            calls[0].0 = 25;
        });
        assert_eq!(report.changed, 2, "{report:?}");
        assert!(out.contains("TARGET_ALL"));
        assert_eq!(out.matches('\n').count(), out.matches("\r\n").count(), "{out:?}");
        assert_eq!(out.len(), text.len() - "LAST".len() + "ALL".len());
        let moved = calls(&out);
        assert_eq!((moved[0].0, moved[1].0), (20, 25), "{out:?}");

        let shared = text.replace(
            "-1);\r\n",
            "-1);\r\n        macros::FT_MOTION_RATE(agent, 0.5);\r\n",
        );
        let (out, _) = rewrite(&shared, |calls| calls[0].0 = 14);
        assert_eq!(out.matches('\n').count(), out.matches("\r\n").count(), "{out:?}");
        assert_eq!(calls(&out)[0].0, 14, "{out:?}");
    }

    /// A move is refused, with the reason, where frame blocks do not say when a line runs.
    #[test]
    fn move_refused_after_a_wait() {
        let text = SPECIAL_N.replace(
            "    frame(agent.lua_state_agent, 20.0);\n",
            "    wait(agent.lua_state_agent, 2.0);\n    frame(agent.lua_state_agent, 20.0);\n",
        );
        let (out, report) = rewrite(&text, |calls| calls[0].0 = 14);
        assert_eq!(out, text);
        assert_eq!(report.changed, 0);
        assert!(report.skipped[0].contains("wait"), "{report:?}");
    }
}
