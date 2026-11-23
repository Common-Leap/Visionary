//! Moving hitboxes in time inside a linked moveset's own source.
//!
//! A hitbox's frames are not arguments of its call: they are where the call sits in the
//! script's `frame(…)` / `wait(…)` sequence, and the `AttackModule::clear`, `clear_all` or
//! same-id `ATTACK` that ends it. So a retime is a structural edit, made here with the same
//! promise as the value writers: the user's lines are moved verbatim and nothing else is
//! restyled.
//!
//! The script is read as a flat timeline of top-level statements, following the frame cursor
//! through `frame` (absolute) and `wait` (relative). A call moves to the execute block already
//! at its new frame, or a new block is made there; a new frame that falls inside a `wait`
//! splits it (`wait(2)` → `wait(1)`, block, `wait(1)`), so every other line keeps the frame it
//! runs on. A script whose timing runs through a loop or a branch is refused rather than
//! restructured. Every result is re-parsed and must time each hitbox exactly as the editor
//! does, or nothing structural is written.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use crate::acmd_src::{format_float, matching_brace, opens_is_excute, skip_trivia, statement_end};
use crate::data::{is_attack_category, script_frame, Hitbox};

/// A hitbox the editor retimed, and where its call is.
pub(crate) struct Retimed<'a> {
    /// Its index in the pristine and edited lists.
    pub position: usize,
    /// Span of the call expression (`macros::ATTACK(…)`).
    pub call: Range<usize>,
    pub before: &'a Hitbox,
    pub after: &'a Hitbox,
}

const NO_END: u32 = 9999;

/// How a hitbox's end is written, which decides what a retime may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// `ATTACK`s and `ATTACK_FP`: ended by `AttackModule::clear`, `clear_all`, or the next call
    /// with their id.
    Attack,
    /// `CATCH`: ended by `GrabModule::clear_all` or the next `CATCH` with its id.
    Grab,
    /// `SEARCH` and `ATTACK_ABS`: no ACMD call ends them — the status code does — so only their
    /// start is the script's to move.
    Fixed,
}

fn family(category: u8) -> Family {
    if is_attack_category(category) {
        Family::Attack
    } else if category == 1 {
        Family::Grab
    } else {
        Family::Fixed
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Frame,
    Wait,
    Excute { open: usize, close: usize },
    Other,
    /// Control flow with timing inside: the cursor after it is not known statically.
    Timed,
}

#[derive(Debug, Clone)]
struct Item {
    /// The whole lines the statement occupies, when it has them to itself.
    lines: Option<Range<usize>>,
    kind: Kind,
    /// The `frame`/`wait` argument's span.
    arg: Range<usize>,
    before: f32,
    after: f32,
}

struct Timeline {
    items: Vec<Item>,
    /// The function's closing brace.
    close: usize,
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |n| n + 1)
}

fn line_end(text: &str, at: usize) -> usize {
    text[at..].find('\n').map_or(text.len(), |n| at + n + 1)
}

/// The whole lines `span` occupies, when nothing but whitespace or a trailing comment shares
/// them.
fn own_lines(text: &str, span: &Range<usize>) -> Option<Range<usize>> {
    let start = line_start(text, span.start);
    if !text[start..span.start].trim().is_empty() {
        return None;
    }
    let end = line_end(text, span.end.saturating_sub(1).max(span.start));
    let tail = text[span.end..end].trim();
    (tail.is_empty() || tail.starts_with("//")).then_some(start..end)
}

/// The index just past a block-led statement (`if … { } else { }`, `for … { }`).
fn block_statement_end(text: &str, start: usize, limit: usize) -> Option<(usize, usize)> {
    let open_at = |from: usize| -> Option<usize> {
        let bytes = text.as_bytes();
        let mut depth = 0usize;
        let mut i = from;
        while i < limit {
            if let Some(next) = skip_trivia(text, i) {
                i = next;
                continue;
            }
            match bytes[i] {
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth = depth.saturating_sub(1),
                b'{' if depth == 0 => return Some(i),
                _ => {}
            }
            i += 1;
        }
        None
    };
    let first_open = open_at(start)?;
    let mut close = matching_brace(text, first_open)?;
    loop {
        let mut i = close + 1;
        while i < limit {
            if let Some(next) = skip_trivia(text, i).filter(|_| text[i..].starts_with('/')) {
                i = next;
            } else if text.as_bytes()[i].is_ascii_whitespace() {
                i += 1;
            } else {
                break;
            }
        }
        if !text[i..limit].starts_with("else") {
            return Some((first_open, close + 1));
        }
        close = matching_brace(text, open_at(i)?)?;
    }
}

/// Whether `text` calls `frame`, `wait` or a `wait_loop_*` itself.
fn has_timing(text: &str) -> bool {
    let code = crate::acmd_src::without_comments(text);
    let bytes = code.as_bytes();
    ["frame(", "wait(", "wait_loop"].iter().any(|word| {
        code.match_indices(word).any(|(at, _)| {
            at == 0 || !(bytes[at - 1].is_ascii_alphanumeric() || matches!(bytes[at - 1], b'_' | b':' | b'.'))
        })
    })
}

/// The second argument of `name(lua_state, N)` at `start`, and its value.
fn timing_arg(text: &str, start: usize, end: usize) -> Option<(Range<usize>, f32)> {
    let open = start + text[start..end].find('(')?;
    let close = open + text[open..end].rfind(')')?;
    let comma = open + text[open..close].find(',')?;
    let raw = &text[comma + 1..close];
    let lead = raw.len() - raw.trim_start().len();
    let value_text = raw.trim();
    let value = value_text.trim_end_matches("f32").parse::<f32>().ok()?;
    let at = comma + 1 + lead;
    Some((at..at + value_text.len(), value))
}

fn parse(text: &str) -> Result<Timeline, String> {
    let header = text.find("fn ").ok_or("no function header")?;
    let open = header + text[header..].find('{').ok_or("no function body")?;
    let close = matching_brace(text, open).ok_or("unbalanced function body")?;
    let mut items = Vec::new();
    let mut cursor = 0.0f32;
    let mut i = open + 1;
    while i < close {
        let rest = &text[i..close];
        let Some(ch) = rest.chars().next() else { break };
        if ch.is_whitespace() || ch == ';' {
            i += ch.len_utf8();
            continue;
        }
        if rest.starts_with("//") || rest.starts_with("/*") {
            i = skip_trivia(text, i).unwrap_or(close);
            continue;
        }
        let start = i;
        let block_led = ["if ", "if(", "for ", "while ", "loop ", "loop{", "match ", "unsafe ", "unsafe{", "{", "fn ", "pub fn "]
            .iter()
            .any(|keyword| rest.starts_with(keyword));
        let (end, kind) = if block_led {
            let (first_open, end) =
                block_statement_end(text, start, close).ok_or("an unbalanced block")?;
            let body = &text[start..end];
            let kind = if has_timing(body) {
                Kind::Timed
            } else if opens_is_excute(&text[start..line_end(text, start)])
                && matching_brace(text, first_open) == Some(end - 1)
            {
                Kind::Excute { open: first_open, close: end - 1 }
            } else {
                Kind::Other
            };
            (end, kind)
        } else {
            let end = statement_end(text, start).map_or(close, |semi| semi + 1);
            let body = &text[start..end];
            let kind = if body.starts_with("frame(") {
                Kind::Frame
            } else if body.starts_with("wait(") {
                Kind::Wait
            } else if has_timing(body) {
                Kind::Timed
            } else {
                Kind::Other
            };
            (end, kind)
        };
        let mut item = Item {
            lines: own_lines(text, &(start..end)),
            kind,
            arg: 0..0,
            before: cursor,
            after: cursor,
        };
        if matches!(kind, Kind::Frame | Kind::Wait) {
            match timing_arg(text, start, end) {
                Some((arg, value)) => {
                    item.arg = arg;
                    item.after = if kind == Kind::Frame { value } else { cursor + value };
                }
                None => item.kind = Kind::Timed,
            }
        }
        cursor = item.after;
        items.push(item);
        i = end;
    }
    Ok(Timeline { items, close })
}

/// How the file spells what this module writes.
struct Style {
    newline: &'static str,
    indent: String,
    unit: String,
    agent: String,
    attack_module: String,
    excute_open: String,
    frame_line: Option<(String, Range<usize>)>,
}

impl Style {
    fn detect(text: &str, timeline: &Timeline) -> Self {
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let indent_of = |at: usize| {
            let start = line_start(text, at);
            let line = &text[start..];
            line[..line.len() - line.trim_start().len()].to_string()
        };
        let first = timeline.items.iter().find_map(|item| item.lines.clone());
        let indent = first.as_ref().map_or_else(|| "    ".to_string(), |lines| indent_of(lines.start));
        let unit = timeline
            .items
            .iter()
            .find_map(|item| match item.kind {
                Kind::Excute { open, close } => {
                    let body_start = line_end(text, open);
                    text[body_start..close].lines().find(|l| !l.trim().is_empty()).map(|l| {
                        let inner = &l[..l.len() - l.trim_start().len()];
                        inner.strip_prefix(indent.as_str()).unwrap_or("    ").to_string()
                    })
                }
                _ => None,
            })
            .filter(|unit| !unit.is_empty())
            .unwrap_or_else(|| "    ".into());
        let agent = text
            .find("fn ")
            .and_then(|at| text[at..].find('(').map(|open| at + open + 1))
            .map(|start| {
                let rest = text[start..].trim_start();
                rest[..rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(0)]
                    .to_string()
            })
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "agent".into());
        let attack_module = text
            .find("AttackModule::")
            .map(|at| {
                let path_start = text[..at]
                    .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':'))
                    .map_or(0, |n| n + 1);
                text[path_start..at + "AttackModule::".len()].to_string()
            })
            .unwrap_or_else(|| "AttackModule::".into());
        let excute_open = timeline
            .items
            .iter()
            .find_map(|item| match item.kind {
                Kind::Excute { open, .. } => Some(text[line_start(text, open)..=open].trim().to_string()),
                _ => None,
            })
            .unwrap_or_else(|| format!("if macros::is_excute({agent}) {{"));
        let frame_line = timeline.items.iter().find_map(|item| {
            let lines = item.lines.clone()?;
            (item.kind == Kind::Frame).then(|| {
                let line = text[lines.clone()].trim_end().to_string();
                let lead = line.len() - line.trim_start().len();
                let arg = item.arg.start - lines.start - lead..item.arg.end - lines.start - lead;
                (line.trim_start().to_string(), arg)
            })
        });
        Style { newline, indent, unit, agent, attack_module, excute_open, frame_line }
    }

    fn frame(&self, frame: u32) -> String {
        match &self.frame_line {
            Some((line, arg)) => {
                let value = number_like(&line[arg.clone()], frame as f32);
                format!("{}{}{}", &line[..arg.start], value, &line[arg.end..])
            }
            None => format!("frame({}.lua_state_agent, {frame}.0);", self.agent),
        }
    }

    fn clear(&self, id: u32) -> String {
        format!("{}clear({}.module_accessor, {id}, false);", self.attack_module, self.agent)
    }

    /// An execute block holding `lines`, at statement indentation.
    fn block(&self, lines: &[String]) -> String {
        let nl = self.newline;
        let mut out = format!("{}{}{nl}", self.indent, self.excute_open);
        for line in lines {
            out.push_str(&format!("{}{}{line}{nl}", self.indent, self.unit));
        }
        out.push_str(&format!("{}}}{nl}", self.indent));
        out
    }
}

/// `value` spelled the way `template` spells its number: `11.0` or `11`.
fn number_like(template: &str, value: f32) -> String {
    if template.contains('.') || value.fract() != 0.0 {
        format_float(value)
    } else {
        format!("{}", value as i64)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Request {
    frame: u32,
    /// A clear goes before anything else on its frame; a call after.
    clear: bool,
    line: String,
}

/// Where a request lands. Ordered as the text is written: a block for frame 1 goes in ahead
/// of a new `frame(…)` inserted before the first statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Anchor {
    /// A new block before the first statement, on frame 1.
    AtStart,
    /// Inside the execute block of item `.0`.
    InBlock(usize),
    /// A new execute block after item `.0`, whose cursor is already on the frame.
    After(usize),
    /// A new `frame` + block before the `frame` item `.0`, which jumps past it.
    BeforeFrame(usize),
    /// Splitting the `wait` item `.0`, which steps over it.
    SplitWait(usize),
    /// A new `frame` + block at the end of the function.
    AtEnd,
}

fn resolve(timeline: &Timeline, request: &Request) -> Option<Anchor> {
    let frame = request.frame;
    let items = &timeline.items;
    let blocks: Vec<usize> = (0..items.len())
        .filter(|&i| matches!(items[i].kind, Kind::Excute { .. }) && script_frame(items[i].before) == frame)
        .collect();
    if let Some(&chosen) = if request.clear { blocks.first() } else { blocks.last() } {
        return Some(Anchor::InBlock(chosen));
    }
    if let Some(first) = items.iter().position(|item| script_frame(item.after) == frame) {
        let mut last = first;
        while last + 1 < items.len() && script_frame(items[last + 1].after) == frame {
            last += 1;
        }
        return Some(Anchor::After(last));
    }
    if items.first().is_none_or(|item| script_frame(item.before) == frame) {
        return Some(Anchor::AtStart);
    }
    for (i, item) in items.iter().enumerate() {
        if script_frame(item.before) < frame && frame < script_frame(item.after) {
            return match item.kind {
                Kind::Frame => Some(Anchor::BeforeFrame(i)),
                Kind::Wait => Some(Anchor::SplitWait(i)),
                _ => None,
            };
        }
    }
    let end = items.last().map_or(0.0, |item| item.after);
    (frame > script_frame(end)).then_some(Anchor::AtEnd)
}

/// The line on `frame` that ends `family`'s hitboxes — `AttackModule::clear(…, id, …)` for one
/// id, or with `id` `None` the family's `clear_all` — as (its lines, its text). It may sit in a
/// nested execute block.
fn clear_line(
    text: &str,
    timeline: &Timeline,
    frame: u32,
    family: Family,
    id: Option<u32>,
) -> Option<(Range<usize>, String)> {
    clear_lines(text, timeline, frame, family, id).into_iter().next()
}

/// Every line `clear_line` could return, in order. A script may end its grab boxes twice on one
/// frame — `grab!` and `GrabModule::clear_all` together — and either one left behind still ends
/// them there.
fn clear_lines(
    text: &str,
    timeline: &Timeline,
    frame: u32,
    family: Family,
    id: Option<u32>,
) -> Vec<(Range<usize>, String)> {
    let mut found = Vec::new();
    for item in &timeline.items {
        let Kind::Excute { open, close } = item.kind else { continue };
        if script_frame(item.before) != frame {
            continue;
        }
        let mut at = line_end(text, open);
        while at < close {
            let end = line_end(text, at).min(close);
            let line = text[at..end].trim();
            let matches = match (family, id) {
                (Family::Attack, Some(id)) => crate::acmd::parse_attack_clear(line) == Some(id),
                (Family::Attack, None) => line.contains("clear_all") && !line.contains("GrabModule"),
                (Family::Grab, None) => crate::acmd::is_grab_clear_all(line),
                _ => false,
            };
            if matches && !line.starts_with("//") && line.ends_with(';') {
                found.push((at..line_end(text, at), line.to_string()));
            }
            at = end;
        }
    }
    found
}

/// Whether a block opening on this line only groups what it holds: a nested
/// `if macros::is_excute(agent) {`, which a script inside one already passed, or a bare or
/// `unsafe` block. Moving a line out of one changes nothing but where it is written.
fn transparent_block(line: &str) -> bool {
    let trimmed = line.trim();
    opens_is_excute(line) && trimmed.ends_with('{') || matches!(trimmed, "{" | "unsafe {")
}

/// Whether every block between the execute block opening at `open` and `at` is transparent.
fn only_transparent_blocks(text: &str, open: usize, at: usize) -> bool {
    let bytes = text.as_bytes();
    let mut stack: Vec<usize> = Vec::new();
    let mut i = open + 1;
    while i < at {
        if let Some(next) = skip_trivia(text, i) {
            i = next;
            continue;
        }
        match bytes[i] {
            b'{' => stack.push(i),
            b'}' => {
                stack.pop();
            }
            _ => {}
        }
        i += 1;
    }
    stack.iter().all(|&brace| {
        let start = line_start(text, brace);
        transparent_block(&text[start..line_end(text, brace)])
            && text[brace + 1..line_end(text, brace)].trim().is_empty()
    })
}

/// The code lines between `open` and `close` as whole-line ranges. A transparent nested block
/// is one entry spanning its lines, holding the code lines inside it (flattened).
fn body_lines(text: &str, open: usize, close: usize) -> Vec<(Range<usize>, Vec<Range<usize>>)> {
    let mut out = Vec::new();
    let mut at = line_end(text, open);
    let stop = line_start(text, close);
    while at < stop {
        let end = line_end(text, at);
        let line = &text[at..end];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            at = end;
            continue;
        }
        if transparent_block(line) {
            let brace = at + line.rfind('{').unwrap_or(0);
            if let Some(inner_close) = matching_brace(text, brace) {
                let block_end = line_end(text, inner_close);
                let inner = body_lines(text, brace, inner_close)
                    .into_iter()
                    .flat_map(|(range, nested)| if nested.is_empty() { vec![range] } else { nested })
                    .collect::<Vec<_>>();
                out.push((at..block_end, inner));
                at = block_end;
                continue;
            }
        }
        out.push((at..end, Vec::new()));
        at = end;
    }
    out
}


type Timing = (u8, String, u32, u32, u32, u32);

/// What a retime must preserve about a hitbox. A throw or search box is compared without its
/// end: no ACMD call ends one, and the parser lets a same-id `ATTACK` close it, which a moved
/// attack would otherwise seem to retime.
fn timing(hitbox: &Hitbox) -> Timing {
    let end = if family(hitbox.category) == Family::Fixed { 0 } else { hitbox.active_end };
    (
        hitbox.category,
        hitbox.func.clone(),
        hitbox.id,
        hitbox.part,
        hitbox.active_start,
        end,
    )
}

fn span_label(start: u32, end: u32) -> String {
    match end {
        NO_END => format!("{start} on"),
        end if end == start => format!("{start}"),
        end => format!("{start}–{end}"),
    }
}

/// `text` with `removed` lines taken out and `requests` written in where they land.
fn place(
    text: &str,
    timeline: &Timeline,
    style: &Style,
    mut removed: BTreeSet<(usize, usize)>,
    requests: &BTreeSet<Request>,
) -> Result<String, String> {
    // Group the requests by where they land.
    let mut anchored: BTreeMap<Anchor, BTreeMap<u32, Vec<&Request>>> = BTreeMap::new();
    for request in requests {
        let Some(anchor) = resolve(timeline, request) else {
            return Err(format!("frame {} has no place in the move's timing", request.frame));
        };
        anchored.entry(anchor).or_default().entry(request.frame).or_default().push(request);
    }
    // Clears first on a frame, then calls (`Request` orders `clear: false` first, so sort).
    // A request may be several lines (a spawn and its modifiers), kept together.
    let lines_for = |requests: &[&Request]| -> Vec<String> {
        let mut ordered: Vec<&&Request> = requests.iter().collect();
        ordered.sort_by_key(|r| !r.clear);
        ordered.iter().flat_map(|r| r.line.split('\n').map(str::to_string)).collect()
    };

    let nl = style.newline;
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut targeted: BTreeSet<usize> = BTreeSet::new();
    for (anchor, by_frame) in &anchored {
        match *anchor {
            Anchor::InBlock(item) => {
                targeted.insert(item);
                let Kind::Excute { open, close } = timeline.items[item].kind else { unreachable!() };
                let close_line = line_start(text, close);
                if !text[close_line..close].trim().is_empty() {
                    return Err("an execute block closes on a line with code".into());
                }
                let body_indent = format!("{}{}", style.indent, style.unit);
                for requests in by_frame.values() {
                    let (clears, calls): (Vec<&Request>, Vec<&Request>) =
                        requests.iter().partition(|r| r.clear);
                    if !clears.is_empty() {
                        let text_in: String = clears
                            .iter()
                            .flat_map(|r| r.line.split('\n'))
                            .map(|line| format!("{body_indent}{line}{nl}"))
                            .collect();
                        edits.push((line_end(text, open)..line_end(text, open), text_in));
                    }
                    if !calls.is_empty() {
                        let text_in: String = calls
                            .iter()
                            .flat_map(|r| r.line.split('\n'))
                            .map(|line| format!("{body_indent}{line}{nl}"))
                            .collect();
                        edits.push((close_line..close_line, text_in));
                    }
                }
            }
            Anchor::After(item) => {
                let Some(lines) = timeline.items[item].lines.clone() else {
                    return Err("a statement shares its line with other code".into());
                };
                let block: String = by_frame.values().map(|r| style.block(&lines_for(r))).collect();
                edits.push((lines.end..lines.end, block));
            }
            Anchor::AtStart => {
                let at = timeline.items.first().and_then(|item| item.lines.clone()).map_or_else(
                    || line_start(text, timeline.close),
                    |lines| lines.start,
                );
                let block: String = by_frame.values().map(|r| style.block(&lines_for(r))).collect();
                edits.push((at..at, block));
            }
            Anchor::BeforeFrame(_) | Anchor::AtEnd => {
                let at = match *anchor {
                    Anchor::BeforeFrame(item) => timeline.items[item].lines.clone().map(|l| l.start),
                    _ => Some(line_start(text, timeline.close)),
                };
                let Some(at) = at else {
                    return Err("a statement shares its line with other code".into());
                };
                let mut block = String::new();
                for (frame, requests) in by_frame {
                    block.push_str(&format!("{}{}{nl}", style.indent, style.frame(*frame)));
                    block.push_str(&style.block(&lines_for(requests)));
                }
                edits.push((at..at, block));
            }
            Anchor::SplitWait(item) => {
                let wait = &timeline.items[item];
                let Some(lines) = wait.lines.clone() else {
                    return Err("a wait shares its line with other code".into());
                };
                let template = text[lines.clone()].trim().to_string();
                let lead = text[lines.clone()].len() - text[lines.clone()].trim_start().len();
                let arg = wait.arg.start - lines.start - lead..wait.arg.end - lines.start - lead;
                let wait_line = |step: f32| {
                    let value = number_like(&template[arg.clone()], step);
                    format!("{}{}{}{}{nl}", style.indent, &template[..arg.start], value, &template[arg.end..])
                };
                let mut out = String::new();
                let mut cursor = wait.before;
                for (frame, requests) in by_frame {
                    out.push_str(&wait_line(*frame as f32 - cursor));
                    out.push_str(&style.block(&lines_for(requests)));
                    cursor = *frame as f32;
                }
                out.push_str(&wait_line(wait.after - cursor));
                edits.push((lines, out));
            }
        }
    }

    // A block whose every line moved out goes with them, unless something moves in — and so
    // does a nested execute block whose lines all moved.
    for (i, item) in timeline.items.iter().enumerate() {
        let Kind::Excute { open, close } = item.kind else { continue };
        let gone_line = |range: &Range<usize>| removed.contains(&(range.start, range.end));
        let mut emptied_nested: Vec<Range<usize>> = Vec::new();
        let mut code = false;
        let mut emptied = false;
        for (range, nested) in body_lines(text, open, close) {
            if nested.is_empty() && !transparent_block(&text[range.clone()]) {
                if gone_line(&range) {
                    emptied = true;
                } else {
                    code = true;
                }
            } else if nested.iter().all(gone_line) {
                emptied = true;
                emptied_nested.push(range);
            } else {
                code = true;
                emptied |= nested.iter().any(gone_line);
            }
        }
        let whole = item.lines.clone().filter(|_| emptied && !code && !targeted.contains(&i));
        let drop: Vec<Range<usize>> = match whole {
            Some(lines) => vec![lines],
            None => emptied_nested,
        };
        for range in drop {
            removed.retain(|&(start, end)| !(range.start <= start && end <= range.end));
            removed.insert((range.start, range.end));
        }
    }
    // A `frame`/`wait` left at the very end with nothing after it any more times nothing:
    // it goes too, but only when this edit is what emptied the tail.
    let gone = |item: &Item| item.lines.as_ref().is_some_and(|l| removed.contains(&(l.start, l.end)));
    let tail_inserts = anchored.contains_key(&Anchor::AtEnd);
    if let Some(last_kept) = timeline.items.iter().rposition(|item| {
        !gone(item) && !matches!(item.kind, Kind::Frame | Kind::Wait)
    }) {
        let tail = &timeline.items[last_kept + 1..];
        if !tail_inserts && tail.iter().any(gone) {
            let mut dangling: Vec<(usize, usize)> = Vec::new();
            for item in tail {
                if matches!(item.kind, Kind::Frame | Kind::Wait) {
                    let Some(lines) = item.lines.clone() else { break };
                    // A wait split by this edit is rewritten, not removed.
                    if edits.iter().any(|(span, _)| !span.is_empty() && span.start == lines.start) {
                        dangling.clear();
                        break;
                    }
                    dangling.push((lines.start, lines.end));
                }
            }
            removed.extend(dangling);
        }
    }
    edits.extend(removed.into_iter().map(|(start, end)| (start..end, String::new())));

    // Inserts before removals at one offset; nothing may overlap.
    edits.sort_by_key(|(span, _)| (span.start, !span.is_empty()));
    let mut out = String::with_capacity(text.len() + 256);
    let mut cursor = 0;
    for (span, value) in &edits {
        if span.start < cursor {
            return Err("two moves would rewrite the same lines".into());
        }
        out.push_str(&text[cursor..span.start]);
        out.push_str(value);
        cursor = span.end;
    }
    out.push_str(&text[cursor..]);

    Ok(out)
}

/// The script's timeline, when lines can be moved through it safely.
fn checked_timeline(text: &str) -> Result<Timeline, String> {
    let timeline = parse(text)?;
    if timeline.items.iter().any(|item| item.kind == Kind::Timed) {
        return Err("the move's frames run through a loop or a branch, which source syncing \
                    does not restructure"
            .into());
    }
    if timeline
        .items
        .iter()
        .any(|item| matches!(item.kind, Kind::Frame | Kind::Wait) && (item.after.fract() != 0.0 || item.before.fract() != 0.0))
    {
        return Err("the move steps through fractional frames".into());
    }
    // `wait`s that already passed a later `frame(…)`: the game skips it, the editor's frames do
    // not, and no placement satisfies both.
    if let Some(item) = timeline.items.iter().find(|item| item.kind == Kind::Frame && item.after < item.before) {
        return Err(format!(
            "its `frame({})` comes after `wait`s that already reached frame {}",
            item.after, item.before
        ));
    }
    Ok(timeline)
}

/// `text` (one `game_` function) with `moved` placed on their edited frames, or why not.
///
/// `pristine` is the source's own hitbox list; only the hitboxes in `moved` change timing, to
/// their edited frames, and the result is checked against exactly that.
pub(crate) fn retime_hitboxes(
    text: &str,
    pristine: &[Hitbox],
    moved: &[Retimed],
) -> Result<String, String> {
    let timeline = checked_timeline(text)?;
    let style = Style::detect(text, &timeline);
    let moved_at: BTreeMap<usize, &Retimed> = moved.iter().map(|m| (m.position, m)).collect();
    // What the source must say once this is done: the edited timing for the moved hitboxes,
    // and the source's own for everything else (an added or skipped one is not written).
    let expected: Vec<&Hitbox> = pristine
        .iter()
        .enumerate()
        .map(|(position, before)| moved_at.get(&position).map_or(before, |m| m.after))
        .collect();
    // One id is one hitbox at a time in the game: a second call with it replaces the first.
    for (i, a) in expected.iter().enumerate() {
        for b in &expected[i + 1..] {
            let out = |h: &Hitbox| family(h.category) != Family::Fixed && h.active_start <= h.active_end;
            if out(a)
                && out(b)
                && family(a.category) == family(b.category)
                && a.id == b.id
                && a.active_start <= b.active_end
                && b.active_start <= a.active_end
            {
                let first = a.active_start.max(b.active_start);
                return Err(format!(
                    "hitbox {} would be out on frame {first} together with the other id-{} \
                     hitbox — one id is one hitbox at a time, so one of them has to end first",
                    a.id, a.id
                ));
            }
        }
    }
    let starts_same_id = |id: u32, frame: u32| {
        expected
            .iter()
            .any(|h| is_attack_category(h.category) && h.id == id && h.active_start == frame)
    };
    // Frames a `GrabModule::clear_all` has to end grab boxes on, and the ones already written
    // whose boxes now end elsewhere.
    let mut grab_clears: BTreeSet<u32> = BTreeSet::new();
    let mut grab_alls: BTreeMap<(usize, usize), u32> = BTreeMap::new();

    let mut removed: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut requests: BTreeSet<Request> = BTreeSet::new();
    let mut clear_alls: BTreeMap<(usize, usize), (u32, String)> = BTreeMap::new();
    let item_of = |at: usize| {
        timeline.items.iter().position(|item| match item.kind {
            Kind::Excute { open, close } => open < at && at < close,
            _ => false,
        })
    };

    for m in moved {
        let id = m.before.id;
        let fail = |why: &str| format!("hitbox {id}: {why}");
        if m.before.active_start != m.after.active_start {
            let Some(item) = item_of(m.call.start) else {
                return Err(fail("its call is not in an execute block at the top of the move"));
            };
            let Kind::Excute { open, .. } = timeline.items[item].kind else { unreachable!() };
            if !only_transparent_blocks(text, open, m.call.start) {
                return Err(fail("its call sits inside a condition or a loop"));
            }
            let statement = m.call.start..statement_end(text, m.call.start).map_or(m.call.end, |semi| semi + 1);
            let Some(lines) = own_lines(text, &statement) else {
                return Err(fail("its call shares a line with other code"));
            };
            removed.insert((lines.start, lines.end));
            requests.insert(Request {
                frame: m.after.active_start,
                clear: false,
                line: text[lines].trim().to_string(),
            });
        }
        let (old_end, new_end) = (m.before.active_end, m.after.active_end);
        if old_end == new_end {
            continue;
        }
        match family(m.before.category) {
            Family::Attack => {}
            Family::Fixed => {
                return Err(fail(
                    "a search or throw box is ended by the status code, not by this script, so \
                     source syncing moves only its start",
                ));
            }
            Family::Grab => {
                let ender = old_end.saturating_add(1);
                let enders = clear_lines(text, &timeline, ender, Family::Grab, None);
                if old_end != NO_END && !enders.is_empty() {
                    for (lines, _) in enders {
                        grab_alls.insert((lines.start, lines.end), ender);
                    }
                } else if new_end != NO_END {
                    grab_clears.insert(new_end + 1);
                }
                continue;
            }
        }
        let shorten = |requests: &mut BTreeSet<Request>| {
            if new_end != NO_END && !starts_same_id(id, new_end + 1) {
                requests.insert(Request { frame: new_end + 1, clear: true, line: style.clear(id) });
            }
        };
        if old_end == NO_END {
            shorten(&mut requests);
            continue;
        }
        let ender = old_end + 1;
        if let Some((lines, line)) = clear_line(text, &timeline, ender, Family::Attack, Some(id)) {
            removed.insert((lines.start, lines.end));
            if new_end != NO_END && !starts_same_id(id, new_end + 1) {
                requests.insert(Request { frame: new_end + 1, clear: true, line });
            }
        } else if let Some((lines, line)) = clear_line(text, &timeline, ender, Family::Attack, None) {
            clear_alls.insert((lines.start, lines.end), (ender, line));
        } else if new_end < old_end {
            // Replaced by the next call with its id: end it earlier with a clear of its own.
            shorten(&mut requests);
        }
        // Lengthening one the next same-id call replaces needs that call moved too; if the
        // editor did not move it, the check below says so.
    }

    // A `clear_all` ends every hitbox open on its frame. It moves to the latest end any of them
    // now has, and each that ends sooner gets a clear of its own — unless on its new frame it
    // would also end a hitbox it never did. Then it gives way to one clear per hitbox.
    for ((start, end), (frame, line)) in clear_alls {
        let members: Vec<usize> = pristine
            .iter()
            .enumerate()
            .filter(|(_, h)| is_attack_category(h.category) && h.active_end + 1 == frame)
            .filter(|(_, h)| clear_line(text, &timeline, frame, Family::Attack, Some(h.id)).is_none())
            .map(|(position, _)| position)
            .collect();
        let latest = members.iter().map(|&p| expected[p].active_end).max().unwrap_or(frame - 1);
        let mut closes_at = frame;
        if latest != frame - 1 {
            removed.insert((start, end));
            closes_at = latest.saturating_add(1);
            let bystander = expected.iter().enumerate().any(|(p, h)| {
                is_attack_category(h.category)
                    && !members.contains(&p)
                    && h.active_start <= closes_at
                    && closes_at <= h.active_end
            });
            if latest == NO_END || bystander {
                // Every member is then ended by its own clear below.
                closes_at = u32::MAX;
            } else {
                requests.insert(Request { frame: closes_at, clear: true, line });
            }
        }
        for &p in &members {
            let h = expected[p];
            if h.active_end != NO_END && h.active_end + 1 < closes_at && !starts_same_id(h.id, h.active_end + 1) {
                requests.insert(Request { frame: h.active_end + 1, clear: true, line: style.clear(h.id) });
            }
        }
    }

    // A hitbox the editor left alone can still lose what ended it: the next same-id call that
    // replaced it moved away. It keeps its frames with a clear of its own.
    let pristine_refs: Vec<&Hitbox> = pristine.iter().collect();
    for (p, h) in expected.iter().enumerate() {
        let end = h.active_end;
        let fam = family(h.category);
        if fam == Family::Fixed || end == NO_END || end != pristine[p].active_end {
            continue;
        }
        let kept = |found: Option<(Range<usize>, String)>| {
            found.is_some_and(|(lines, _)| !removed.contains(&(lines.start, lines.end)))
        };
        let replaced_in = |list: &[&Hitbox]| {
            list.iter().any(|o| family(o.category) == fam && o.id == h.id && o.active_start == end + 1)
        };
        if !replaced_in(&pristine_refs) || replaced_in(&expected) {
            continue;
        }
        if fam == Family::Attack {
            if !kept(clear_line(text, &timeline, end + 1, fam, Some(h.id)))
                && !kept(clear_line(text, &timeline, end + 1, fam, None))
            {
                requests.insert(Request { frame: end + 1, clear: true, line: style.clear(h.id) });
            }
        } else if !kept(clear_line(text, &timeline, end + 1, fam, None)) {
            grab_clears.insert(end + 1);
        }
    }

    // Grab boxes have no per-id clear a script can call: one `GrabModule::clear_all` ends them
    // all. One that is written moves to the frames its boxes now end on; a new one goes in only
    // where it ends exactly the boxes the editor ends there.
    for ((start, end), frame) in grab_alls {
        let members: Vec<usize> = pristine
            .iter()
            .enumerate()
            .filter(|(_, h)| family(h.category) == Family::Grab && h.active_end + 1 == frame)
            .map(|(p, _)| p)
            .collect();
        if members.iter().all(|&p| expected[p].active_end + 1 == frame) {
            continue;
        }
        removed.insert((start, end));
        grab_clears.extend(
            members
                .iter()
                .map(|&p| expected[p].active_end)
                .filter(|&e| e != NO_END)
                .map(|e| e + 1),
        );
    }
    let grab_line = text
        .lines()
        .map(str::trim)
        .find(|line| crate::acmd::is_grab_clear_all(line) && !line.starts_with("//") && line.ends_with(';'))
        .map(str::to_string)
        .unwrap_or_else(|| format!("GrabModule::clear_all({}.module_accessor);", style.agent));
    for frame in grab_clears {
        let caught = expected.iter().find(|h| {
            family(h.category) == Family::Grab && h.active_start < frame && frame <= h.active_end
        });
        if let Some(h) = caught {
            return Err(format!(
                "grab box {} would be ended on frame {frame} too — one GrabModule::clear_all ends \
                 every grab box, so they cannot end on different frames",
                h.id
            ));
        }
        requests.insert(Request { frame, clear: true, line: grab_line.clone() });
    }

    let out = place(text, &timeline, &style, removed, &requests)?;

    let mut want: Vec<Timing> = expected.iter().map(|h| timing(h)).collect();
    let mut got: Vec<Timing> = crate::acmd::parse_acmd_script(&out).to_hitboxes().iter().map(timing).collect();
    want.sort();
    got.sort();
    if want != got {
        let missing = want.iter().find(|t| !got.contains(t));
        let reason = match missing {
            Some((category, func, id, part, start, end)) => {
                let instead = got
                    .iter()
                    .filter(|g| (&g.0, &g.1, g.2, g.3) == (category, func, *id, *part) && !want.contains(g))
                    .map(|g| span_label(g.4, g.5))
                    .next();
                let blocker = expected.iter().find(|h| {
                    is_attack_category(h.category) && h.id == *id && h.active_start > *start && h.active_start <= *end
                });
                match blocker {
                    Some(next) => format!(
                        "hitbox {id} would run frames {}, but the next id-{id} call replaces it \
                         on frame {} — retime that one too",
                        span_label(*start, *end),
                        next.active_start
                    ),
                    None => match instead {
                        Some(instead) => format!(
                            "hitbox {id} would run frames {instead} instead of {} — the frames \
                             around it end it differently",
                            span_label(*start, *end)
                        ),
                        None => format!(
                            "the rewritten source would not run hitbox {id} on frames {}",
                            span_label(*start, *end)
                        ),
                    },
                }
            }
            None => "the rewritten source would change another hitbox's frames".into(),
        };
        return Err(reason);
    }
    Ok(out)
}

/// A two-armed conditional inside an execute block: `if COND { … } else { … }`, the shape a
/// directional effect takes (`PostureModule::lr(…) < 0.0` picks the left or right graphic).
struct Chain {
    /// Its whole lines.
    lines: Range<usize>,
    /// Each arm's braces, the `if` arm first.
    arms: Vec<(usize, usize)>,
}

/// The conditional directly inside an execute block (through transparent blocks only) whose arm
/// holds `at` directly, if `at` is in one.
fn enclosing_chain(text: &str, timeline: &Timeline, at: usize) -> Result<Option<Chain>, String> {
    let item = timeline
        .items
        .iter()
        .find(|item| matches!(item.kind, Kind::Excute { open, close } if open < at && at < close))
        .ok_or("its call is not in an execute block at the top of the move")?;
    let Kind::Excute { open, close } = item.kind else { unreachable!() };
    if only_transparent_blocks(text, open, at) {
        return Ok(None);
    }
    // The statements of the execute block, and of any transparent block in it.
    let mut i = line_end(text, open);
    let mut limit = close;
    while i < limit {
        let rest = &text[i..limit];
        let Some(ch) = rest.chars().next() else { break };
        if ch.is_whitespace() || ch == ';' {
            i += ch.len_utf8();
            continue;
        }
        if rest.starts_with("//") || rest.starts_with("/*") {
            i = skip_trivia(text, i).unwrap_or(limit);
            continue;
        }
        let start = i;
        let line = &text[line_start(text, start)..line_end(text, start)];
        let block_led = rest.starts_with("if ") || rest.starts_with("if(") || rest.starts_with('{') || rest.starts_with("unsafe");
        let end = if block_led {
            block_statement_end(text, start, limit).ok_or("an unbalanced block")?.1
        } else {
            statement_end(text, start).map_or(limit, |semi| semi + 1)
        };
        if !(start <= at && at < end) {
            i = end;
            continue;
        }
        if transparent_block(line) {
            // Descend into the transparent block.
            let brace = start + text[start..end].find('{').ok_or("an unbalanced block")?;
            i = brace + 1;
            limit = end - 1;
            continue;
        }
        if !(rest.starts_with("if ") || rest.starts_with("if(")) {
            return Err("its call sits inside a block that is not a plain condition".into());
        }
        let (first_open, _) = block_statement_end(text, start, limit).ok_or("an unbalanced block")?;
        let first_close = matching_brace(text, first_open).ok_or("an unbalanced block")?;
        let mut arms = vec![(first_open, first_close)];
        let after = text[first_close + 1..end].trim_start();
        if let Some(rest) = after.strip_prefix("else") {
            let rest = rest.trim_start();
            if !rest.starts_with('{') {
                return Err("its call sits in an `else if` chain, which source syncing does not split".into());
            }
            let else_open = first_close + 1 + text[first_close + 1..end].find('{').ok_or("an unbalanced block")?;
            arms.push((else_open, matching_brace(text, else_open).ok_or("an unbalanced block")?));
        }
        let Some(&(arm_open, _)) = arms.iter().find(|(o, c)| *o < at && at < *c) else {
            return Err("its call sits in a condition's header".into());
        };
        if !only_transparent_blocks(text, arm_open, at) {
            return Err("its call sits inside a condition or a loop".into());
        }
        let lines = own_lines(text, &(start..end)).ok_or("its condition shares a line with other code")?;
        return Ok(Some(Chain { lines, arms }));
    }
    Err("its call is not in an execute block at the top of the move".into())
}

/// An effect spawn the editor retimed: its call and the `LAST_EFFECT_SET_*` lines that modify
/// it, which run against the last spawn and so have to move with it.
pub(crate) struct EffectRetimed<'a> {
    /// The spawn's call first, then its modifiers', in source order.
    pub calls: Vec<Range<usize>>,
    pub before: &'a crate::data::EffectCall,
    pub after: &'a crate::data::EffectCall,
}

fn is_trail(call: &crate::data::EffectCall) -> bool {
    call.trail_command.is_some() || call.spawn_func == "AFTER_IMAGE_ON" || call.raw_line.is_some()
}

type EffectTiming = (String, String, u32, u32);

fn effect_timing(call: &crate::data::EffectCall) -> EffectTiming {
    let end = if call.follows_bone || is_trail(call) { call.active_end } else { 0 };
    (call.spawn_func.clone(), call.effect_name.to_ascii_lowercase(), call.active_start, end)
}

/// The statement lines `span`'s call owns, after checking it can be moved.
fn movable_lines(text: &str, timeline: &Timeline, span: &Range<usize>) -> Result<Range<usize>, String> {
    let item = timeline
        .items
        .iter()
        .find(|item| matches!(item.kind, Kind::Excute { open, close } if open < span.start && span.start < close))
        .ok_or("its call is not in an execute block at the top of the move")?;
    let Kind::Excute { open, .. } = item.kind else { unreachable!() };
    if !only_transparent_blocks(text, open, span.start) && enclosing_chain(text, timeline, span.start)?.is_none() {
        return Err("its call sits inside a condition or a loop".into());
    }
    let statement = span.start..statement_end(text, span.start).map_or(span.end, |semi| semi + 1);
    own_lines(text, &statement).ok_or_else(|| "its call shares a line with other code".into())
}

/// The one stop line that ends `call` on `frame`: `EFFECT_OFF_KIND` naming it, or for a trail
/// `AFTER_IMAGE_OFF`.
fn stop_line(
    text: &str,
    timeline: &Timeline,
    call: &crate::data::EffectCall,
    frame: u32,
) -> Result<Range<usize>, String> {
    let quoted = format!("\"{}\"", call.effect_name.to_ascii_lowercase());
    let mut found = Vec::new();
    for item in &timeline.items {
        let Kind::Excute { open, close } = item.kind else { continue };
        if script_frame(item.before) != frame {
            continue;
        }
        let mut at = line_end(text, open);
        while at < close {
            let end = line_end(text, at).min(close);
            let line = text[at..end].trim();
            let stops = if is_trail(call) {
                line.contains("AFTER_IMAGE_OFF(")
            } else {
                line.contains("EFFECT_OFF_KIND(") && line.to_ascii_lowercase().contains(&quoted)
            };
            if stops && !line.starts_with("//") {
                found.push(at..line_end(text, at));
            }
            at = end;
        }
    }
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(format!("no stop for it on frame {frame} — source syncing does not add or remove stops")),
        _ => Err(format!("more than one stop on frame {frame} could be its own")),
    }
}

/// Whether code after `from`, up to the next spawn, still acts on "the last effect" — a
/// costume- or facing-conditional `LAST_EFFECT_SET_COLOR`, or an `EffectModule::set_scale_last`
/// a few frames on, which the parser keeps as verbatim lines. "Last" means the last one spawned,
/// whatever frame it is now, so those belong to the spawn above them and would bind to another
/// one if it moved without them.
fn modified_after(text: &str, _timeline: &Timeline, spawns: &[Range<usize>], from: usize) -> bool {
    let stop = spawns
        .iter()
        .map(|s| s.start)
        .filter(|&start| start >= from)
        .min()
        .unwrap_or(text.len());
    touches_last_effect(&crate::acmd_src::without_comments(&text[from..stop.max(from)]))
}

/// The whole lines a retimed spawn takes with it: its own call, its typed modifiers, and every
/// line between or straight after them in its block that acts on "the last effect"
/// (`EffectModule::set_scale_last`, which no typed field holds). Those run against whatever was
/// spawned last, so left behind they would land on another effect. Any other line among them
/// keeps its frame, so the spawn cannot move without it and the move is refused.
fn effect_group(
    text: &str,
    timeline: &Timeline,
    spawns: &[Range<usize>],
    calls: &[Range<usize>],
) -> Result<Vec<Range<usize>>, String> {
    let mut group: Vec<Range<usize>> = calls
        .iter()
        .map(|span| movable_lines(text, timeline, span))
        .collect::<Result<_, _>>()?;
    group.sort_by_key(|r| r.start);
    group.dedup();
    let first = group.first().map(|r| r.start).ok_or("no source call")?;
    let block_close = timeline
        .items
        .iter()
        .find_map(|item| match item.kind {
            Kind::Excute { open, close } if open < first && first < close => Some(close),
            _ => None,
        })
        .ok_or("its call is not in an execute block at the top of the move")?;
    let next_spawn = |from: usize| {
        spawns.iter().map(|s| s.start).filter(|&start| start >= from).min().unwrap_or(text.len())
    };
    let mut at = first;
    let mut out = Vec::new();
    let last_typed = group.last().map_or(first, |r| r.end);
    let stop = line_start(text, block_close);
    while at < stop {
        let end = line_end(text, at).min(stop);
        let range = at..end;
        let line = crate::acmd_src::without_comments(&text[range.clone()]);
        let trimmed = line.trim();
        if group.contains(&range) {
            out.push(range);
        } else if trimmed.is_empty() {
            // Blank or comment: stays where it is.
        } else if at >= last_typed && (at >= next_spawn(last_typed) || !touches_last_effect(trimmed)) {
            // Past its own lines: the next spawn's, or code of its own.
            break;
        } else if touches_last_effect(trimmed)
            && trimmed.ends_with(';')
            && trimmed.matches('{').count() == trimmed.matches('}').count()
        {
            out.push(range);
        } else {
            return Err("a line between it and its modifiers would be left behind and act on another effect".into());
        }
        at = end;
    }
    Ok(out)
}

/// Whether `code` acts on the last spawned effect rather than naming one.
pub(crate) fn touches_last_effect(code: &str) -> bool {
    ["LAST_EFFECT", "LAST_PARTICLE", "_last(", "last_handle"]
        .iter()
        .any(|token| code.contains(token))
}

/// `text` (one `effect_` function) with `moved` spawns placed on their edited frames, or why
/// not. Each spawn moves with its modifiers; a follow effect's or trail's stop moves with its
/// end. Checked against the full edited timing the way [`retime_hitboxes`] is.
pub(crate) fn retime_effects(
    text: &str,
    spawns: &[Range<usize>],
    edited: &[crate::data::EffectCall],
    moved: &[EffectRetimed],
) -> Result<String, String> {
    use crate::data::OPEN_ENDED_EFFECT_FRAME;
    let timeline = checked_timeline(text)?;
    let style = Style::detect(text, &timeline);
    let mut removed: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut requests: BTreeSet<Request> = BTreeSet::new();
    // Conditionals already moved whole, by their first line.
    let mut moved_chains: BTreeSet<usize> = BTreeSet::new();
    for m in moved {
        let name = &m.before.effect_name;
        let fail = |why: String| format!("`{name}`: {why}");
        if m.before.active_start != m.after.active_start {
            let spawn = m.calls.first().ok_or_else(|| fail("no source call".into()))?;
            let chain = enclosing_chain(text, &timeline, spawn.start).map_err(fail)?;
            match chain {
                None => {
                    let group = effect_group(text, &timeline, spawns, &m.calls).map_err(fail)?;
                    let group_end = group.iter().map(|r| r.end).max().unwrap_or(spawn.end);
                    let mut lines = Vec::new();
                    for range in &group {
                        removed.insert((range.start, range.end));
                        lines.push(text[range.clone()].trim().to_string());
                    }
                    if modified_after(text, &timeline, spawns, group_end) {
                        return Err(fail(
                            "lines after it still change the last effect it spawned (a \
                             conditional color or rate), and would bind to another effect"
                                .into(),
                        ));
                    }
                    requests.insert(Request { frame: m.after.active_start, clear: false, line: lines.join("\n") });
                }
                Some(chain) => {
                    let inside = |span: &Range<usize>| chain.lines.start <= span.start && span.start < chain.lines.end;
                    // Every spawn in the conditional moving to one frame moves the conditional.
                    let whole = spawns.iter().filter(|s| inside(s)).all(|s| {
                        moved.iter().any(|o| {
                            o.calls.first() == Some(s) && o.after.active_start == m.after.active_start
                        })
                    });
                    if whole && modified_after(text, &timeline, spawns, chain.lines.end) {
                        return Err(fail(
                            "lines after its condition still change the last effect it spawned"
                                .into(),
                        ));
                    }
                    if whole {
                        if moved_chains.insert(chain.lines.start) {
                            removed.insert((chain.lines.start, chain.lines.end));
                            let body: Vec<&str> = text[chain.lines.clone()].lines().collect();
                            let lead = body.first().map_or(0, |l| l.len() - l.trim_start().len());
                            let line = body
                                .iter()
                                .map(|l| if l.len() >= lead && l[..lead].trim().is_empty() { &l[lead..] } else { l.trim_start() })
                                .map(|l| l.trim_end_matches('\r'))
                                .collect::<Vec<_>>()
                                .join("\n");
                            requests.insert(Request { frame: m.after.active_start, clear: false, line });
                        }
                    } else {
                        // Only this arm's spawn moves: it keeps its condition at the new frame,
                        // written with the arm's own header lines so the spawn keeps the guard
                        // the editor knows it by. An `else` arm keeps its `if` too, emptied.
                        let is_else = chain.arms.get(1).is_some_and(|&(o, c)| o < spawn.start && spawn.start < c);
                        let (if_open, if_close) = chain.arms[0];
                        let header_lines = |from: usize, to: usize| -> Vec<String> {
                            text[line_start(text, from)..line_end(text, to)]
                                .lines()
                                .map(|l| l.trim().to_string())
                                .filter(|l| !l.is_empty())
                                .collect()
                        };
                        let mut lines = header_lines(if_open, if_open);
                        if is_else {
                            let (else_open, _) = chain.arms[1];
                            lines.extend(header_lines(if_close, else_open));
                        }
                        let arm_close = chain
                            .arms
                            .iter()
                            .find(|&&(o, c)| o < spawn.start && spawn.start < c)
                            .map_or(spawn.end, |&(_, c)| c);
                        if m.calls.iter().any(|c| c.end > arm_close) {
                            return Err(fail(
                                "a modifier after its condition changes whichever arm ran, so one arm cannot move alone"
                                    .into(),
                            ));
                        }
                        let group = effect_group(text, &timeline, spawns, &m.calls).map_err(fail)?;
                        let last_call = group.iter().map(|r| r.end).max().unwrap_or(spawn.end);
                        if modified_after(text, &timeline, spawns, last_call) {
                            return Err(fail(
                                "lines after it still change the last effect it spawned, and would bind to another effect"
                                    .into(),
                            ));
                        }
                        for range in &group {
                            removed.insert((range.start, range.end));
                            lines.push(format!("{}{}", style.unit, text[range.clone()].trim()));
                        }
                        lines.push("}".into());
                        requests.insert(Request { frame: m.after.active_start, clear: false, line: lines.join("\n") });
                    }
                }
            }
        }
        let ends = m.before.follows_bone || is_trail(m.before);
        if ends && m.before.active_end != m.after.active_end {
            if m.before.active_end == OPEN_ENDED_EFFECT_FRAME || m.after.active_end == OPEN_ENDED_EFFECT_FRAME {
                return Err(fail("it would need a stop added or removed, which source syncing does not do".into()));
            }
            let range = stop_line(text, &timeline, m.before, m.before.active_end).map_err(fail)?;
            // One `EFFECT_OFF_KIND` ends every effect of its kind that is out.
            let sharing = edited
                .iter()
                .filter(|c| !std::ptr::eq(*c, m.after))
                .filter(|c| (c.follows_bone || is_trail(c)) == (m.before.follows_bone || is_trail(m.before)))
                .filter(|c| is_trail(m.before) || c.effect_name.eq_ignore_ascii_case(name))
                .filter(|c| c.active_start <= m.before.active_end && c.active_end == m.before.active_end)
                .count();
            if sharing > 0 {
                return Err(fail(format!(
                    "its stop on frame {} also ends {sharing} other effect(s) of its kind — move their ends with it",
                    m.before.active_end
                )));
            }
            removed.insert((range.start, range.end));
            requests.insert(Request {
                frame: m.after.active_end,
                clear: false,
                line: text[range].trim().to_string(),
            });
        }
    }
    let out = place(text, &timeline, &style, removed, &requests)?;
    let mut want: Vec<EffectTiming> = edited.iter().filter(|c| !c.disabled).map(effect_timing).collect();
    let mut got: Vec<EffectTiming> = crate::acmd::parse_effect_script(&out)
        .to_effect_calls()
        .into_iter()
        .map(crate::data::EffectCall::normalized_timing)
        .map(|c| effect_timing(&c))
        .collect();
    want.sort();
    got.sort();
    if want != got {
        let missing = want.iter().find(|t| !got.contains(t));
        return Err(match missing {
            Some((_, name, start, _)) => format!(
                "the rewritten source would not spawn `{name}` on frame {start} as the editor does"
            ),
            None => "the rewritten source would change another effect's frames".into(),
        });
    }
    Ok(out)
}

/// An effect the editor added: the lines that spawn it (the call and its modifiers), on its
/// frame, inside its condition when it has one (`if PostureModule::lr(…) < 0.0 {`).
pub(crate) struct EffectAddition {
    pub frame: u32,
    pub lines: Vec<String>,
    pub guard: Option<String>,
}

/// `text` (one `effect_` function) with `additions` written in on their frames, following the
/// script's `frame`/`wait` timeline the way a retime does: into the execute block already on the
/// frame, a new one after the statements that reach it, or a split `wait` that steps over it.
/// Every other line keeps the frame it runs on. A script whose timing runs through a loop or a
/// branch is refused — which arm a new spawn belongs in is not the editor's to guess.
pub(crate) fn add_effects(text: &str, additions: &[EffectAddition]) -> Result<String, String> {
    let timeline = checked_timeline(text)?;
    let style = Style::detect(text, &timeline);
    let mut requests: BTreeSet<Request> = BTreeSet::new();
    for addition in additions {
        if addition.lines.is_empty() {
            continue;
        }
        let line = match &addition.guard {
            Some(header) => {
                let mut lines = vec![header.trim().to_string()];
                lines.extend(addition.lines.iter().map(|l| format!("{}{}", style.unit, l.trim())));
                lines.push("}".into());
                lines.join("\n")
            }
            None => addition.lines.iter().map(|l| l.trim()).collect::<Vec<_>>().join("\n"),
        };
        requests.insert(Request { frame: addition.frame.max(1), clear: false, line });
    }
    place(text, &timeline, &style, BTreeSet::new(), &requests)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hitboxes(source: &str) -> Vec<Hitbox> {
        crate::acmd::parse_acmd_script(source).to_hitboxes()
    }

    fn attack(id: u32, damage: &str) -> String {
        format!(
            "macros::ATTACK(agent, {id}, 0, Hash40::new(\"top\"), {damage}, 50, 90, 0, 50, 3.5, 7.5, 0.5, 0.0, None, None, None, 1.2, 1.0, *ATTACK_SETOFF_KIND_ON, *ATTACK_LR_CHECK_POS, false, 0, 0.0, 0, false, false, false, false, true, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_ALL, *COLLISION_PART_MASK_ALL, false, Hash40::new(\"collision_attr_normal\"), *ATTACK_SOUND_LEVEL_L, *COLLISION_SOUND_ATTR_PUNCH, *ATTACK_REGION_PUNCH);"
        )
    }

    /// Empoleon's utilt, shape for shape: `frame(11)`, a `wait(2)` to the late hit, `clear_all`
    /// on 22.
    fn utilt() -> String {
        format!(
            "unsafe extern \"C\" fn game_attackhi3(agent: &mut L2CAgentBase) {{
    frame(agent.lua_state_agent, 11.0);
    if macros::is_excute(agent) {{
        {}
        {}
    }}
    wait(agent.lua_state_agent, 2.0);
    if macros::is_excute(agent) {{
        {}
        {}
    }}
    frame(agent.lua_state_agent, 22.0);
    if macros::is_excute(agent) {{
        AttackModule::clear_all(agent.module_accessor);
    }}
}}
",
            attack(0, "12.5"),
            attack(1, "12.5"),
            attack(0, "7.4"),
            attack(1, "8.9")
        )
    }

    fn retime(source: &str, change: impl Fn(&mut Vec<Hitbox>)) -> Result<String, String> {
        let pristine = hitboxes(source);
        let mut edited = pristine.clone();
        change(&mut edited);
        let sites: Vec<Range<usize>> = crate::acmd_src::scan_macro_sites(source, 0..source.len())
            .into_iter()
            .filter(|site| site.name == "ATTACK")
            .map(|site| site.span)
            .collect();
        let moved: Vec<Retimed> = (0..pristine.len())
            .filter(|&p| timing(&pristine[p]) != timing(&edited[p]))
            .map(|p| Retimed { position: p, call: sites[p].clone(), before: &pristine[p], after: &edited[p] })
            .collect();
        let out = retime_hitboxes(source, &pristine, &moved)?;
        let mut got: Vec<Timing> = hitboxes(&out).iter().map(timing).collect();
        let mut want: Vec<Timing> = edited.iter().map(timing).collect();
        got.sort();
        want.sort();
        assert_eq!(got, want, "{out}");
        Ok(out)
    }

    #[test]
    fn utilt_parses_as_the_editor_shows_it() {
        let spans: Vec<(u32, u32)> = hitboxes(&utilt()).iter().map(|h| (h.active_start, h.active_end)).collect();
        assert_eq!(spans, vec![(11, 12), (11, 12), (13, 21), (13, 21)]);
    }

    #[test]
    fn an_earlier_start_moves_the_calls_and_keeps_the_wait_landing_where_it_did() {
        let out = retime(&utilt(), |h| {
            h[0].active_start = 9;
            h[1].active_start = 9;
        })
        .unwrap();
        assert!(out.contains("frame(agent.lua_state_agent, 9.0);"), "{out}");
        // The emptied frame-11 block is gone; frame 11 still anchors the wait to 13.
        assert!(out.contains("frame(agent.lua_state_agent, 11.0);\n    wait("), "{out}");
    }

    #[test]
    fn a_start_inside_a_wait_splits_it() {
        let out = retime(&utilt(), |h| {
            h[2].active_start = 12;
            h[0].active_end = 11;
        })
        .unwrap();
        assert!(out.contains("wait(agent.lua_state_agent, 1.0);"), "{out}");
        assert_eq!(out.matches("wait(agent.lua_state_agent, 1.0);").count(), 2, "{out}");
    }

    #[test]
    fn a_shorter_hit_gets_its_own_clear() {
        let out = retime(&utilt(), |h| h[2].active_end = 17).unwrap();
        assert!(out.contains("frame(agent.lua_state_agent, 18.0);"), "{out}");
        assert!(out.contains("AttackModule::clear(agent.module_accessor, 0, false);"), "{out}");
        assert!(out.contains("frame(agent.lua_state_agent, 22.0);"), "id 1 still ends on 22: {out}");
    }

    /// Every hitbox a `clear_all` ends ends sooner: the `clear_all` itself moves, and the
    /// frame it leaves behind goes with it.
    #[test]
    fn shortening_everything_a_clear_all_ends_moves_it() {
        let out = retime(&utilt(), |h| {
            h[2].active_end = 17;
            h[3].active_end = 17;
        })
        .unwrap();
        assert!(out.contains("frame(agent.lua_state_agent, 18.0);"), "{out}");
        assert_eq!(out.matches("clear_all").count(), 1, "{out}");
        assert!(!out.contains("22.0"), "{out}");
        assert!(!out.contains("AttackModule::clear("), "{out}");
    }

    #[test]
    fn a_longer_hit_moves_the_clear_all_and_clears_the_rest_where_they_ended() {
        let out = retime(&utilt(), |h| h[2].active_end = 25).unwrap();
        assert!(out.contains("frame(agent.lua_state_agent, 26.0);"), "{out}");
        assert!(out.contains("AttackModule::clear(agent.module_accessor, 1, false);"), "{out}");
        assert_eq!(out.matches("clear_all").count(), 1, "{out}");
    }

    #[test]
    fn shifting_a_whole_hit_later_keeps_the_late_hit() {
        retime(&utilt(), |h| {
            for b in h.iter_mut().take(2) {
                b.active_start = 12;
            }
        })
        .unwrap();
    }

    /// The late hit moves a frame later: the early hit it used to replace keeps its frames.
    #[test]
    fn a_hitbox_whose_replacement_moved_away_keeps_its_end() {
        let out = retime(&utilt(), |h| {
            h[2].active_start = 14;
            h[3].active_start = 14;
        })
        .unwrap();
        assert!(out.contains("AttackModule::clear(agent.module_accessor, 0, false);"), "{out}");
        assert!(out.contains("AttackModule::clear(agent.module_accessor, 1, false);"), "{out}");
    }

    /// Falco's dsmash shape: a `clear_all` on 15 between two hits. Lengthening the first hit
    /// past 15 must not let that `clear_all` land on the second hit.
    #[test]
    fn a_clear_all_that_would_land_on_another_hit_becomes_clears() {
        let source = format!(
            "unsafe extern \"C\" fn game_attacklw4(agent: &mut L2CAgentBase) {{
    frame(agent.lua_state_agent, 13.0);
    if macros::is_excute(agent) {{
        {}
        {}
    }}
    frame(agent.lua_state_agent, 15.0);
    if macros::is_excute(agent) {{
        AttackModule::clear_all(agent.module_accessor);
    }}
    frame(agent.lua_state_agent, 16.0);
    if macros::is_excute(agent) {{
        {}
    }}
    frame(agent.lua_state_agent, 20.0);
    if macros::is_excute(agent) {{
        AttackModule::clear_all(agent.module_accessor);
    }}
}}
",
            attack(0, "3.5"),
            attack(1, "3.5"),
            attack(2, "9.0")
        );
        // id 1 now runs to 17, through the id-2 hit that starts on 16.
        let out = retime(&source, |h| h[1].active_end = 17).unwrap();
        assert!(out.contains("AttackModule::clear(agent.module_accessor, 0, false);"), "{out}");
        assert!(out.contains("AttackModule::clear(agent.module_accessor, 1, false);"), "{out}");
        assert_eq!(out.matches("clear_all").count(), 1, "{out}");
    }

    /// Samus's landing: several boxes from frame 1, one `clear_all` on 22, and one box made to
    /// last a frame longer.
    #[test]
    fn one_box_outlasting_a_shared_clear_all() {
        let source = format!(
            "unsafe extern \"C\" fn game_landingairlw(agent: &mut L2CAgentBase) {{
    frame(agent.lua_state_agent, 1.0);
    if macros::is_excute(agent) {{
        {}
        {}
    }}
    frame(agent.lua_state_agent, 2.0);
    if macros::is_excute(agent) {{
        macros::SET_SPEED_EX(agent, 1.7, 0.0, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);
    }}
    wait(agent.lua_state_agent, 5.0);
    frame(agent.lua_state_agent, 22.0);
    if macros::is_excute(agent) {{
        AttackModule::clear_all(agent.module_accessor);
    }}
}}
",
            attack(0, "3.2"),
            attack(1, "3.2")
        );
        let out = match retime(&source, |h| h[0].active_end = 22) {
            Ok(out) => out,
            Err(e) => panic!("{e}"),
        };
        assert!(out.contains("frame(agent.lua_state_agent, 23.0);"), "{out}");
    }

    /// A jab hit moved to frame 1 and cleared on 2, ahead of a script whose first statement is
    /// `frame(3)`: both new blocks go before it, frame 1's first.
    #[test]
    fn moving_to_frame_one_ahead_of_the_first_frame() {
        let out = retime(&utilt(), |h| {
            h[0].active_start = 1;
            h[0].active_end = 1;
        })
        .unwrap();
        let one = out.find(&attack(0, "12.5")).unwrap();
        let two = out.find("frame(agent.lua_state_agent, 2.0);").unwrap();
        assert!(one < two, "{out}");
    }

    #[test]
    fn lengthening_into_the_next_same_id_call_says_why_not() {
        let error = retime(&utilt(), |h| h[0].active_end = 15).unwrap_err();
        assert!(error.contains("out on frame 13 together with the other id-0 hitbox"), "{error}");
    }

    #[test]
    fn timing_inside_a_loop_is_refused() {
        let source = format!(
            "unsafe extern \"C\" fn game_attackairb(agent: &mut L2CAgentBase) {{
    for _ in 0..2 {{
        if macros::is_excute(agent) {{
            {}
        }}
        wait(agent.lua_state_agent, 6.0);
    }}
}}
",
            attack(0, "1.0")
        );
        let error = retime(&source, |h| h[0].active_start = 3).unwrap_err();
        assert!(error.contains("loop or a branch"), "{error}");
    }

    /// Through the real writer, so every family is matched to its own call.
    fn sync(source: &str, change: impl Fn(&mut Vec<Hitbox>)) -> (String, crate::acmd_src::SyncReport) {
        let pristine = hitboxes(source);
        let mut edited = pristine.clone();
        change(&mut edited);
        let (out, report) = crate::acmd_src::rewrite_hitboxes(source, "t", &pristine, &edited).unwrap();
        if report.skipped.is_empty() {
            let mut got: Vec<Timing> = hitboxes(&out).iter().map(timing).collect();
            let mut want: Vec<Timing> = edited.iter().map(timing).collect();
            got.sort();
            want.sort();
            assert_eq!(got, want, "{out}");
        }
        (out, report)
    }

    /// Conquest's forward smash: its late hit sits in `if is_excute { if is_excute { … } }`.
    #[test]
    fn a_hit_inside_a_redundant_nested_execute_block_moves() {
        let source = format!(
            "unsafe extern \"C\" fn game_attacks4(agent: &mut L2CAgentBase) {{
    frame(agent.lua_state_agent, 29.0);
    if macros::is_excute(agent) {{
        {}
    }}
    wait(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {{
    if macros::is_excute(agent) {{
        {}
    }}
    }}
    wait(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {{
        AttackModule::clear_all(agent.module_accessor);
    }}
}}
",
            attack(0, "20.0"),
            attack(0, "12.5")
        );
        let (out, report) = sync(&source, |h| {
            h[1].active_start = 34;
            h[0].active_end = 33;
        });
        assert!(report.skipped.is_empty(), "{report:?}\n{out}");
        // Both execute blocks the late hit sat in are gone with it.
        assert_eq!(out.matches("is_excute").count(), 3, "{out}");
    }

    const GRAB: &str = "unsafe extern \"C\" fn game_catch(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 7.0);
    if macros::is_excute(agent) {
        macros::CATCH(agent, 0, Hash40::new(\"top\"), 3.3, 0.0, 8.0, 4.0, Some(0.0), Some(8.0), Some(11.7), *FIGHTER_STATUS_KIND_CAPTURE_PULLED, *COLLISION_SITUATION_MASK_GA);
    }
    wait(agent.lua_state_agent, 2.0);
    if macros::is_excute(agent) {
        grab!(agent, *MA_MSC_CMD_GRAB_CLEAR_ALL);
        GrabModule::clear_all(agent.module_accessor);
    }
}
";

    #[test]
    fn a_grab_box_moves_and_its_clear_all_follows_its_end() {
        let (out, report) = sync(GRAB, |h| {
            h[0].active_start = 6;
            h[0].active_end = 10;
        });
        assert!(report.skipped.is_empty(), "{report:?}\n{out}");
        assert!(out.contains("frame(agent.lua_state_agent, 6.0);"), "{out}");
        assert_eq!(grab_span(&out), (6, 10), "{out}");
    }

    /// What vanilla and nearly every moveset write: the msc command, with no `GrabModule` call.
    const GRAB_MSC: &str = "unsafe extern \"C\" fn game_catch(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 7.0);
    if macros::is_excute(agent) {
        macros::CATCH(agent, 0, Hash40::new(\"top\"), 3.3, 0.0, 8.0, 4.0, Some(0.0), Some(8.0), Some(11.7), *FIGHTER_STATUS_KIND_CAPTURE_PULLED, *COLLISION_SITUATION_MASK_GA);
    }
    wait(agent.lua_state_agent, 2.0);
    if macros::is_excute(agent) {
        grab!(agent, *MA_MSC_CMD_GRAB_CLEAR_ALL);
    }
}
";

    fn grab_span(source: &str) -> (u32, u32) {
        let hitboxes = crate::acmd::parse_acmd_script(source).to_hitboxes();
        let grab = hitboxes.iter().find(|h| h.category == 1).unwrap();
        (grab.active_start, grab.active_end)
    }

    #[test]
    fn a_grab_ended_by_the_msc_command_reads_its_end() {
        assert_eq!(grab_span(GRAB_MSC), (7, 8));
    }

    #[test]
    fn lengthening_a_grab_moves_its_msc_clear() {
        let (out, report) = sync(GRAB_MSC, |h| h[0].active_end = 12);
        assert!(report.skipped.is_empty(), "{report:?}\n{out}");
        assert_eq!(grab_span(&out), (7, 12), "{out}");
        assert_eq!(out.matches("MA_MSC_CMD_GRAB_CLEAR_ALL").count(), 1, "{out}");
        assert!(!out.contains("GrabModule"), "{out}");
    }

    #[test]
    fn shortening_a_grab_moves_its_msc_clear() {
        let (out, report) = sync(GRAB_MSC, |h| h[0].active_end = 7);
        assert!(report.skipped.is_empty(), "{report:?}\n{out}");
        assert_eq!(grab_span(&out), (7, 7), "{out}");
    }

    #[test]
    fn lengthening_a_grab_moves_both_of_its_clears() {
        let (out, report) = sync(GRAB, |h| h[0].active_end = 12);
        assert!(report.skipped.is_empty(), "{report:?}\n{out}");
        assert_eq!(grab_span(&out), (7, 12), "{out}");
        // Neither clear may stay on the old frame, where it would still end the box.
        assert!(!out.contains("frame(agent.lua_state_agent, 9.0)"), "{out}");
    }

    #[test]
    fn a_search_box_moves_its_start_but_not_its_end() {
        let source = "unsafe extern \"C\" fn game_specialn(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 9.0);
    if macros::is_excute(agent) {
        macros::SEARCH(agent, 0, 0, Hash40::new(\"top\"), 7.38, 0.0, 16.59, 10.38, Some(0.0), Some(10.15), Some(10.38), *COLLISION_KIND_MASK_ATTACK, *HIT_STATUS_MASK_NORMAL, 60, *COLLISION_SITUATION_MASK_GA, *COLLISION_CATEGORY_MASK_FIGHTER, *COLLISION_PART_MASK_ALL, false);
    }
}
";
        let (out, report) = sync(source, |h| h[0].active_start = 8);
        assert!(report.skipped.is_empty(), "{report:?}\n{out}");
        assert!(out.contains("frame(agent.lua_state_agent, 8.0);"), "{out}");
        let (_, report) = sync(source, |h| h[0].active_end = 20);
        assert!(report.skipped.iter().any(|s| s.contains("ended by the status code")), "{report:?}");
    }

    fn effect_sync(
        source: &str,
        change: impl Fn(&mut Vec<crate::data::EffectCall>),
    ) -> (String, crate::acmd_src::SyncReport) {
        let pristine = crate::acmd::parse_effect_script(source).to_effect_calls();
        let mut edited = pristine.clone();
        change(&mut edited);
        crate::acmd_src::rewrite_effect_calls(source, "t", &pristine, &edited).unwrap()
    }

    fn starts(source: &str) -> Vec<(String, u32, u32)> {
        crate::acmd::parse_effect_script(source)
            .to_effect_calls()
            .into_iter()
            .map(|c| (c.effect_name, c.active_start, c.active_end))
            .collect()
    }

    /// Effects in a script stepped by `wait`, the shape the block mover refuses.
    const EFFECTS: &str = "unsafe extern \"C\" fn effect_attackhi3(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 10.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW(agent, Hash40::new(\"sys_attack_arc\"), Hash40::new(\"top\"), 0, 12, 0, 0, 0, 0, 1.0, true);
        macros::LAST_EFFECT_SET_RATE(agent, 1.5);
        macros::FOOT_EFFECT(agent, Hash40::new(\"sys_run_smoke\"), Hash40::new(\"top\"), -3, 0, 0, 0, 0, 0, 1.3, 0, 0, 0, 0, 0, 0, false);
    }
    wait(agent.lua_state_agent, 6.0);
    if macros::is_excute(agent) {
        macros::EFFECT_OFF_KIND(agent, Hash40::new(\"sys_attack_arc\"), false, false);
    }
}
";

    #[test]
    fn an_effect_moves_through_a_wait_with_its_modifier_and_its_stop() {
        let (out, report) = effect_sync(EFFECTS, |calls| {
            calls[0].active_start = 12;
            calls[0].active_end = 18;
        });
        assert!(report.skipped.is_empty(), "{report:?}\n{out}");
        let got = starts(&out);
        assert!(got.contains(&("sys_attack_arc".into(), 12, 18)), "{got:?}\n{out}");
        assert!(got.iter().any(|(name, start, _)| name == "sys_run_smoke" && *start == 10), "{out}");
        // The rate still follows the spawn it modifies.
        let rate = out.find("LAST_EFFECT_SET_RATE").unwrap();
        let spawn = out.find("sys_attack_arc").unwrap();
        assert!(spawn < rate && !out[spawn..rate].contains("FOOT_EFFECT"), "{out}");
    }

    /// Conquest's directional effects: one spawn per facing, in `if lr < 0 { … } else { … }`.
    fn facing(rest: &str) -> String {
        format!(
            "unsafe extern \"C\" fn effect_attackairf(agent: &mut L2CAgentBase) {{
    frame(agent.lua_state_agent, 12.0);
    if macros::is_excute(agent) {{
        if PostureModule::lr(agent.module_accessor) < 0.0 {{ //left
        macros::EFFECT_FOLLOW_FLIP(agent, Hash40::new(\"sys_attack_line\"), Hash40::new(\"sys_attack_line\"), Hash40::new(\"top\"), 9.0, 15.0, -4.0, 0, 0, 0, 1.0, true, *EF_FLIP_YZ);
        macros::LAST_EFFECT_SET_COLOR(agent, 2.35, 1.03, 0.35);
        }}
        else {{ //right
        macros::EFFECT_FOLLOW_FLIP(agent, Hash40::new(\"sys_attack_line\"), Hash40::new(\"sys_attack_line\"), Hash40::new(\"top\"), 9.0, 15.0, -4.0, 0, 0, 0, 1.0, true, *EF_FLIP_YZ);
        macros::LAST_EFFECT_SET_COLOR(agent, 0.93, 1.03, 1.96);
        }}
    }}
{rest}}}
"
        )
    }

    #[test]
    fn both_arms_of_a_facing_condition_move_together() {
        let source = facing("");
        let (out, report) = effect_sync(&source, |calls| {
            for call in calls.iter_mut() {
                call.active_start = 14;
            }
        });
        assert!(report.skipped.is_empty(), "{report:?}\n{out}");
        assert!(starts(&out).iter().all(|(_, start, _)| *start == 14), "{out}");
        assert_eq!(out.matches("PostureModule::lr").count(), 1, "{out}");
    }

    #[test]
    fn one_arm_of_a_facing_condition_keeps_its_condition() {
        let source = facing("");
        let (out, report) = effect_sync(&source, |calls| calls[1].active_start = 14);
        assert!(report.skipped.is_empty(), "{report:?}\n{out}");
        let got = starts(&out);
        assert!(got.iter().any(|(_, start, _)| *start == 12) && got.iter().any(|(_, start, _)| *start == 14), "{out}");
        // The right-facing spawn still runs only when facing right.
        let moved = out.rfind("frame(agent.lua_state_agent, 14.0);").unwrap();
        assert!(out[moved..].contains("else { //right"), "{out}");
    }

    /// A conditional color after a spawn applies to it; moving the spawn away would hand the
    /// color to another effect, so the move is refused and the values are kept.
    #[test]
    fn a_conditional_modifier_after_a_spawn_keeps_it_in_place() {
        let source = "unsafe extern \"C\" fn effect_attack11(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 4.0);
    if macros::is_excute(agent) {
        macros::EFFECT(agent, Hash40::new(\"sys_attack_impact\"), Hash40::new(\"top\"), 0, 8, 6, 0, 0, 0, 1.0, 0, 0, 0, 0, 0, 0, false);
        if WorkModule::get_int(agent.module_accessor, *FIGHTER_INSTANCE_WORK_ID_INT_COLOR) == 7 {
            macros::LAST_EFFECT_SET_COLOR(agent, 1.0, 0.2, 0.2);
        }
    }
    wait(agent.lua_state_agent, 3.0);
}
";
        let (out, report) = effect_sync(source, |calls| calls[0].active_start = 5);
        assert!(!report.skipped.is_empty(), "{out}");
        assert_eq!(out, source);
    }

    /// Empoleon's neutral special: `EffectModule::set_scale_last` between the beam and its
    /// rate is not a typed modifier, but it acts on the beam all the same, so it moves with it.
    /// The move's frames run through a `wait`, which is split around the new frame.
    #[test]
    fn an_untyped_last_effect_line_moves_with_its_spawn() {
        let source = "unsafe extern \"C\" fn effect_specialnstart(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 34.0);
    if macros::is_excute(agent) {
        macros::EFFECT_FOLLOW_FLIP(agent, Hash40::new(\"empoleon_beam\"), Hash40::new(\"empoleon_beam\"), Hash40::new(\"top\"), 0, 6.5, 18, 90, 0, 0, 0.3, true, *EF_FLIP_XY);
        EffectModule::set_scale_last(agent.module_accessor, &Vector3f{x:0.3,y:0.065,z:0.5} );
        macros::LAST_EFFECT_SET_RATE(agent, 4.0);
        macros::EFFECT_FOLLOW_FLIP(agent, Hash40::new(\"empoleon_blizzard\"), Hash40::new(\"empoleon_blizzard\"), Hash40::new(\"top\"), 0, 6.5, 18, 90, 0, 0, 0.20, false, *EF_FLIP_XY);
        macros::LAST_EFFECT_SET_RATE(agent, 4.0);
    }
    wait(agent.lua_state_agent, 15.0);
    if macros::is_excute(agent) {
        macros::EFFECT_OFF_KIND(agent, Hash40::new(\"empoleon_beam\"), false, false);
        macros::EFFECT_OFF_KIND(agent, Hash40::new(\"empoleon_blizzard\"), false, false);
    }
}
";
        let (out, report) = effect_sync(source, |calls| {
            calls[0].active_start = 35;
            calls[0].active_end = 50;
        });
        assert!(report.skipped.is_empty(), "{report:?}\n{out}");
        let got = starts(&out);
        assert!(got.contains(&("empoleon_beam".into(), 35, 50)), "{got:?}\n{out}");
        assert!(got.contains(&("empoleon_blizzard".into(), 34, 49)), "{got:?}\n{out}");
        let beam = out.find("empoleon_beam\"), Hash40::new(\"empoleon_beam").unwrap();
        let scale = out.find("set_scale_last").unwrap();
        assert!(beam < scale && !out[beam..scale].contains("blizzard"), "{out}");
    }

    /// The dash attack shape: hits on 15 and 34, reached by `wait`. An effect added in Visionary
    /// on 20 (between them) and ended early on 26 is written into the user's own source, and a
    /// facing-guarded one keeps its condition.
    #[test]
    fn an_effect_added_in_visionary_is_written_into_a_wait_stepped_script() {
        let source = "unsafe extern \"C\" fn effect_attackdash(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 15.0);
    if macros::is_excute(agent) {
        macros::FOOT_EFFECT(agent, Hash40::new(\"sys_dash_smoke\"), Hash40::new(\"top\"), -3, 0, 0, 0, 0, 0, 1.3, 0, 0, 0, 0, 0, 0, false);
    }
    wait(agent.lua_state_agent, 19.0);
    if macros::is_excute(agent) {
        macros::LANDING_EFFECT(agent, Hash40::new(\"sys_landing_smoke\"), Hash40::new(\"top\"), 0, 0, 0, 0, 0, 0, 0.9, 0, 0, 0, 0, 0, 0, false);
    }
}
";
        let pristine = crate::acmd::parse_effect_script(source).to_effect_calls();
        let mut edited = pristine.clone();
        // A follow effect from 20, ended on 26 — neither frame is in the script.
        let mut added = pristine[0].clone();
        added.effect_name = "sys_attack_line".into();
        added.spawn_func = "EFFECT_FOLLOW".into();
        added.follows_bone = true;
        added.extra_args = Some(vec!["true".into()]);
        added.active_start = 20;
        added.active_end = 26;
        edited.push(added);
        let (out, report) = crate::acmd_src::rewrite_effect_calls(source, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}\n{out}");
        let got = starts(&out);
        assert!(got.contains(&("sys_attack_line".into(), 20, 26)), "{got:?}\n{out}");
        assert!(got.contains(&("sys_dash_smoke".into(), 15, 15)), "{got:?}\n{out}");
        assert!(got.contains(&("sys_landing_smoke".into(), 34, 34)), "{got:?}\n{out}");
        // The wait was split around both new frames, not replaced by absolute frames.
        assert!(out.contains("wait(agent.lua_state_agent, 5.0);"), "{out}");
        assert!(out.contains("EFFECT_OFF_KIND"), "{out}");
    }

    /// The same follow effect with an early stop, added to a script with no `wait` at all. Its
    /// stop is written with the default `EFFECT_OFF_KIND` arguments, which the read-back once took
    /// for a different call, so no effect made in Visionary ever synced.
    #[test]
    fn an_added_follow_effect_with_a_stop_syncs_into_a_frame_only_script() {
        let source = "unsafe extern \"C\" fn effect_attackdash(agent: &mut L2CAgentBase) {
    frame(agent.lua_state_agent, 15.0);
    if macros::is_excute(agent) {
        macros::FOOT_EFFECT(agent, Hash40::new(\"sys_dash_smoke\"), Hash40::new(\"top\"), -3, 0, 0, 0, 0, 0, 1.3, 0, 0, 0, 0, 0, 0, false);
    }
    frame(agent.lua_state_agent, 34.0);
    if macros::is_excute(agent) {
        macros::LANDING_EFFECT(agent, Hash40::new(\"sys_landing_smoke\"), Hash40::new(\"top\"), 0, 0, 0, 0, 0, 0, 0.9, 0, 0, 0, 0, 0, 0, false);
    }
}
";
        let pristine = crate::acmd::parse_effect_script(source).to_effect_calls();
        let mut edited = pristine.clone();
        let mut added = pristine[0].clone();
        added.effect_name = "sys_attack_line".into();
        added.spawn_func = "EFFECT_FOLLOW".into();
        added.follows_bone = true;
        added.extra_args = Some(vec!["true".into()]);
        added.active_start = 20;
        added.active_end = 26;
        edited.push(added);
        let (out, report) = crate::acmd_src::rewrite_effect_calls(source, "t", &pristine, &edited).unwrap();
        assert!(report.skipped.is_empty(), "{report:?}\n{out}");
        assert!(starts(&out).contains(&("sys_attack_line".into(), 20, 26)), "{out}");
    }

    #[test]
    fn crlf_and_style_are_kept() {
        let source = utilt().replace('\n', "\r\n");
        let out = retime(&source, |h| {
            h[0].active_start = 9;
            h[1].active_start = 9;
        })
        .unwrap();
        assert!(!out.replace("\r\n", "").contains('\n'), "{out:?}");
    }
}
