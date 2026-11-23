//! A project's own ACMD helper functions that stand for one known macro call.
//!
//! Some movesets wrap smash_script's macros to shorten the calls they write all the time. Shy
//! Guy's `common/macros.rs`, for one:
//!
//! ```text
//! pub unsafe fn RUMBLE_HIT(agent: &mut L2CAgentBase, kind: Hash40) {
//!     smash_script::macros::RUMBLE_HIT(agent, kind, 0);
//! }
//! pub unsafe fn RUMBLE_HIT_FOR(agent: &mut L2CAgentBase, kind: Hash40, duration_frames: u64) {
//!     smash_script::macros::RUMBLE_HIT(agent, kind, duration_frames);
//! }
//! ```
//!
//! A call to one of these — `macros::RUMBLE_HIT(agent, rb!("attackm"))` — has the wrong shape
//! for the vanilla parser, so it read as a raw line. This module reads each helper as *target
//! macro + argument mapping* and applies that one mapping on both sides of the editor:
//!
//! - [`ProjectHelpers::inline`] shows the call as the macro it runs, in the editor's copy of the
//!   script, so the parser types it;
//! - while a [`HelperScope`] is open, the write-back scanner presents the same call as that
//!   macro too: arguments the author passes keep their real spans, and arguments the helper
//!   fixes get an empty span. Call numbering therefore never drifts between reading and writing,
//!   and an edit to a fixed argument is refused (see [`refuse_fixed`]) instead of written.
//!
//! Only helpers whose whole body is one upper-case macro call qualify. Anything with logic of
//! its own — a random pick, a lua-stack push — stays a raw line, which is always safe.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

/// Where one argument of the target macro comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Slot {
    /// The helper call's own argument at this index (0 is the agent).
    Param(usize),
    /// Fixed by the helper's body.
    Fixed(String),
    /// `PARAM.module_accessor` — the receiver of a module call, built from that argument.
    Accessor(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Helper {
    /// Parameters, the agent included.
    pub params: usize,
    /// What it calls: a macro name without its path (`RUMBLE_HIT`), or one of the module calls
    /// in [`MODULE_TARGETS`] (`ControlModule::set_rumble`).
    pub target: String,
    pub slots: Vec<Slot>,
}

/// Module calls the parsers type directly, and so can stand behind a helper. A helper's own
/// `RUMBLE(agent, kind, frames)` around `ControlModule::set_rumble(agent.module_accessor, kind,
/// frames, false, *BATTLE_OBJECT_ID_INVALID as u32)` is Shy Guy's.
pub const MODULE_TARGETS: &[&str] = &["ControlModule::set_rumble"];

/// Helpers by `(name, parameter count)`.
#[derive(Debug, Default, Clone)]
pub struct ProjectHelpers {
    helpers: HashMap<(String, usize), Helper>,
}

thread_local! {
    static SCOPE: RefCell<Option<Arc<ProjectHelpers>>> = const { RefCell::new(None) };
    static REFUSED: RefCell<usize> = const { RefCell::new(0) };
}

/// While alive, [`crate::acmd_src::scan_macro_sites`] reads helper calls as their target.
pub struct HelperScope(Option<Arc<ProjectHelpers>>);

impl Drop for HelperScope {
    fn drop(&mut self) {
        let previous = self.0.take();
        SCOPE.with(|scope| *scope.borrow_mut() = previous);
    }
}

pub fn scope(helpers: Arc<ProjectHelpers>) -> HelperScope {
    REFUSED.with(|count| *count.borrow_mut() = 0);
    let previous = SCOPE.with(|scope| scope.borrow_mut().replace(helpers));
    HelperScope(previous)
}

/// The helper a scanned `name(…)` call with `args` arguments stands for, in the open scope.
pub(crate) fn in_scope(name: &str, args: usize) -> Option<Helper> {
    SCOPE.with(|scope| {
        scope
            .borrow()
            .as_ref()?
            .helpers
            .get(&(name.to_string(), args))
            .cloned()
    })
}

/// Called when a write would land on an argument a helper fixes (an empty span).
pub(crate) fn refuse_fixed() {
    REFUSED.with(|count| *count.borrow_mut() += 1);
}

/// How many edits were refused because they targeted a helper's fixed argument.
pub fn take_refused() -> usize {
    REFUSED.with(|count| std::mem::take(&mut *count.borrow_mut()))
}

impl ProjectHelpers {
    pub fn scan<'a>(sources: impl IntoIterator<Item = &'a str>) -> Self {
        let mut found: HashMap<(String, usize), Option<Helper>> = HashMap::new();
        for text in sources {
            for (name, helper) in helper_definitions(text) {
                found
                    .entry((name, helper.params))
                    .and_modify(|known| {
                        if known.as_ref() != Some(&helper) {
                            *known = None;
                        }
                    })
                    .or_insert(Some(helper));
            }
        }
        ProjectHelpers {
            helpers: found
                .into_iter()
                .filter_map(|(key, helper)| Some((key, helper?)))
                .collect(),
        }
    }

    /// `body` with every helper call written as the macro call it makes.
    pub fn inline<'a>(&self, body: &'a str) -> Cow<'a, str> {
        if self.helpers.is_empty() {
            return Cow::Borrowed(body);
        }
        let sites = crate::acmd_src::scan_macro_sites_unscoped(body, 0..body.len());
        let mut out = String::new();
        let mut last = 0;
        for site in sites {
            let Some(helper) = self.helpers.get(&(site.name.clone(), site.args.len())) else {
                continue;
            };
            let args: Vec<String> = helper
                .slots
                .iter()
                .map(|slot| match slot {
                    Slot::Param(index) => body[site.args[*index].clone()].to_string(),
                    Slot::Fixed(text) => text.clone(),
                    Slot::Accessor(index) => {
                        format!("{}.module_accessor", body[site.args[*index].clone()].trim())
                    }
                })
                .collect();
            out.push_str(&body[last..site.span.start]);
            let call = if MODULE_TARGETS.contains(&helper.target.as_str()) {
                helper.target.clone()
            } else {
                format!("macros::{}", helper.target)
            };
            out.push_str(&format!("{call}({})", args.join(", ")));
            last = site.span.end;
        }
        if last == 0 {
            return Cow::Borrowed(body);
        }
        out.push_str(&body[last..]);
        Cow::Owned(out)
    }
}

/// `(name, helper)` for every function in `text` that takes the agent first and whose body is a
/// single upper-case macro call built from its parameters and fixed values.
fn helper_definitions(text: &str) -> Vec<(String, Helper)> {
    let mut out = Vec::new();
    let mut search = 0;
    while let Some(rel) = text[search..].find("fn ") {
        let at = search + rel;
        search = at + 3;
        if at > 0 && text.as_bytes()[at - 1].is_ascii_alphanumeric() {
            continue;
        }
        let name_start = at + 3;
        let name_len = text[name_start..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(0);
        let name = &text[name_start..name_start + name_len];
        // ACMD helpers mirror the macros they wrap: upper-case names only.
        if name.len() < 3
            || !name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            continue;
        }
        let Some(open) = text[name_start + name_len..]
            .find('(')
            .map(|n| name_start + name_len + n)
        else {
            continue;
        };
        let Some(close) = close_of(text, open) else {
            continue;
        };
        let params: Vec<String> = split_top(&text[open + 1..close])
            .into_iter()
            .filter_map(|param| {
                param
                    .split_once(':')
                    .map(|(name, _)| name.trim().to_string())
            })
            .collect();
        let first_is_agent = split_top(&text[open + 1..close])
            .first()
            .is_some_and(|first| first.contains("L2CAgentBase"));
        if !first_is_agent || params.is_empty() {
            continue;
        }
        let Some(body_open) = text[close..].find('{').map(|n| close + n) else {
            continue;
        };
        // Nothing but a return type between the parameters and the body.
        let between = text[close + 1..body_open].trim();
        if !(between.is_empty() || between.starts_with("->")) {
            continue;
        }
        let Some(body_close) = close_of(text, body_open) else {
            continue;
        };
        let body = text[body_open + 1..body_close].trim();
        let body = body.strip_suffix(';').unwrap_or(body).trim();
        if body.contains(';') || body.contains('{') {
            continue;
        }
        let Some(call_open) = body.find('(') else {
            continue;
        };
        let path = &body[..call_open];
        if !path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
        {
            continue;
        }
        let module_target = MODULE_TARGETS
            .iter()
            .find(|target| path == **target || path.ends_with(&format!("::{target}")));
        let target = match module_target {
            Some(target) => *target,
            None => path.rsplit("::").next().unwrap_or(path),
        };
        if module_target.is_none()
            && (target.len() < 3
                || !target
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
        {
            continue;
        }
        let Some(call_close) = close_of(body, call_open) else {
            continue;
        };
        if call_close + 1 != body.len() {
            continue;
        }
        let mut used = vec![false; params.len()];
        let slots: Option<Vec<Slot>> = split_top(&body[call_open + 1..call_close])
            .into_iter()
            .map(|arg| {
                let arg = arg.trim();
                if let Some(index) = params.iter().position(|p| p == arg) {
                    if used[index] {
                        return None; // passed twice: no single place to write an edit
                    }
                    used[index] = true;
                    return Some(Slot::Param(index));
                }
                if let Some(receiver) = arg.strip_suffix(".module_accessor") {
                    if let Some(index) = params.iter().position(|p| p == receiver.trim()) {
                        return Some(Slot::Accessor(index));
                    }
                }
                // A fixed value must not depend on any parameter.
                let mentions_param = params.iter().any(|p| contains_word(arg, p));
                (!mentions_param).then(|| Slot::Fixed(arg.to_string()))
            })
            .collect();
        let Some(slots) = slots else { continue };
        let agent_first = match module_target {
            Some(_) => slots.first() == Some(&Slot::Accessor(0)),
            None => slots.first() == Some(&Slot::Param(0)),
        };
        if !agent_first {
            continue; // the agent goes first, untouched
        }
        out.push((
            name.to_string(),
            Helper {
                params: params.len(),
                target: target.to_string(),
                slots,
            },
        ));
        search = body_close;
    }
    out
}

fn contains_word(text: &str, word: &str) -> bool {
    let bytes = text.as_bytes();
    text.match_indices(word).any(|(at, _)| {
        let before = at == 0 || !(bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_');
        let after = bytes
            .get(at + word.len())
            .is_none_or(|b| !(b.is_ascii_alphanumeric() || *b == b'_'));
        before && after
    })
}

fn close_of(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut i = open;
    let mut in_string = false;
    while i < text.len() {
        let b = bytes[i];
        if in_string {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == b'"' {
                in_string = false;
            }
        } else {
            match b {
                b'"' => in_string = true,
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn split_top(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let mut in_string = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if b == b'"' && (i == 0 || bytes[i - 1] != b'\\') {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' | b'>' => depth -= 1,
            b',' if depth == 0 => {
                out.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if !text[start..].trim().is_empty() {
        out.push(&text[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shy Guy's `common/macros.rs`, trimmed to the shapes that matter.
    pub(crate) const MACROS: &str = r#"
/// Rumbles on the next hit.
#[inline]
pub unsafe fn RUMBLE_HIT(agent: &mut L2CAgentBase, kind: Hash40) {
    smash_script::macros::RUMBLE_HIT(agent, kind, 0);
}

/// Rumbles on the next hit, with a custom duration.
#[inline]
pub unsafe fn RUMBLE_HIT_FOR(
    agent: &mut L2CAgentBase,
    kind: Hash40,
    duration_frames: u64,
) {
    smash_script::macros::RUMBLE_HIT(agent, kind, duration_frames);
}

/// Starts a rumble now.
#[inline]
pub unsafe fn RUMBLE(
    agent: &mut L2CAgentBase,
    kind: Hash40,
    duration_frames: i32,
) {
    ControlModule::set_rumble(
        agent.module_accessor,
        kind,
        duration_frames,
        false,
        *BATTLE_OBJECT_ID_INVALID as u32,
    );
}

/// Logic of its own: not a helper this module can see through.
pub unsafe fn PLAY_RANDOM_SE(agent: &mut L2CAgentBase, outcomes: impl RandomSePool) {
    let total_weight = outcomes.total_weight();
    outcomes.play_roll(agent, total_weight);
}
"#;

    #[test]
    fn single_call_helpers_are_read_as_their_target() {
        let helpers = ProjectHelpers::scan([MACROS]);
        assert_eq!(
            helpers.helpers.get(&("RUMBLE_HIT".into(), 2)),
            Some(&Helper {
                params: 2,
                target: "RUMBLE_HIT".into(),
                slots: vec![Slot::Param(0), Slot::Param(1), Slot::Fixed("0".into())],
            })
        );
        assert_eq!(
            helpers
                .helpers
                .get(&("RUMBLE_HIT_FOR".into(), 3))
                .map(|h| &h.slots),
            Some(&vec![Slot::Param(0), Slot::Param(1), Slot::Param(2)])
        );
        assert!(!helpers
            .helpers
            .keys()
            .any(|(name, _)| name == "PLAY_RANDOM_SE"));
        assert_eq!(
            helpers
                .helpers
                .get(&("RUMBLE".into(), 3))
                .map(|h| (&h.target, &h.slots)),
            Some((
                &"ControlModule::set_rumble".to_string(),
                &vec![
                    Slot::Accessor(0),
                    Slot::Param(1),
                    Slot::Param(2),
                    Slot::Fixed("false".into()),
                    Slot::Fixed("*BATTLE_OBJECT_ID_INVALID as u32".into()),
                ]
            ))
        );
        assert_eq!(
            helpers.inline("        macros::RUMBLE(agent, Hash40::new(\"rbkind_jump\"), 0);"),
            "        ControlModule::set_rumble(agent.module_accessor, Hash40::new(\"rbkind_jump\"), 0, false, *BATTLE_OBJECT_ID_INVALID as u32);"
        );
    }

    #[test]
    fn a_helper_call_reads_as_the_macro_it_runs_and_the_vanilla_shape_is_untouched() {
        let helpers = ProjectHelpers::scan([MACROS]);
        let body = "    macros::RUMBLE_HIT(agent, Hash40::new(\"rbkind_attackm\"));\n    macros::RUMBLE_HIT_FOR(agent, Hash40::new(\"rbkind_attackl\"), 6);\n    macros::RUMBLE_HIT(agent, Hash40::new(\"rbkind_x\"), 3);\n";
        assert_eq!(
            helpers.inline(body),
            "    macros::RUMBLE_HIT(agent, Hash40::new(\"rbkind_attackm\"), 0);\n    macros::RUMBLE_HIT(agent, Hash40::new(\"rbkind_attackl\"), 6);\n    macros::RUMBLE_HIT(agent, Hash40::new(\"rbkind_x\"), 3);\n"
        );
    }
}
