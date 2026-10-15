//! The coding conventions of one source file, so text Visionary generates reads as if its author
//! wrote it.
//!
//! Every line the editor inserts into a linked project — a new `frame` block, an added effect, a
//! whole new script — is generated in one canonical spelling: `macros::NAME(agent, …)`,
//! `frame(agent.lua_state_agent, 5.0)`, `if macros::is_excute(agent) {`, four-space indents and
//! `\n` line endings. Real movesets differ on every one of those:
//!
//! - projects that glob-import `smash_script::macros::*` call `ATTACK(agent, …)` bare, and a
//!   `macros::ATTACK` dropped into such a file does not even compile;
//! - plenty name the agent `fighter` (or `weapon`), and an inserted `agent` is an unknown name;
//! - some indent with tabs, and Windows checkouts use `\r\n`.
//!
//! [`SourceStyle::detect`] reads those choices off the file, and [`SourceStyle::apply`] rewrites
//! canonical text into them. It only ever touches generated text; nothing already in the file is
//! re-spelled.

/// How one file spells the things Visionary generates.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceStyle {
    /// The agent parameter's name: `agent`, `fighter`, `weapon`…
    pub agent: String,
    /// What precedes an ACMD macro: `macros::`, `smash_script::macros::`, or nothing.
    pub macro_path: String,
    /// What precedes `is_excute(`.
    pub excute_path: String,
    /// The first argument of `frame(…)` / `wait(…)`, e.g. `agent.lua_state_agent`.
    pub lua_state: String,
    /// Whether frame numbers are written as floats (`5.0`) rather than integers (`5`).
    pub float_frames: bool,
    /// One level of indentation.
    pub indent: String,
    pub newline: &'static str,
}

impl Default for SourceStyle {
    fn default() -> Self {
        SourceStyle {
            agent: "agent".into(),
            macro_path: "macros::".into(),
            excute_path: "macros::".into(),
            lua_state: "agent.lua_state_agent".into(),
            float_frames: true,
            indent: "    ".into(),
            newline: "\n",
        }
    }
}

impl SourceStyle {
    /// Read the conventions off `text` — a function, or a whole file. Anything the text gives no
    /// evidence for keeps the canonical spelling.
    pub fn detect(text: &str) -> Self {
        let mut style = SourceStyle {
            newline: if text.contains("\r\n") { "\r\n" } else { "\n" },
            ..SourceStyle::default()
        };
        // The agent: the first parameter of an ACMD function.
        if let Some(name) = crate::acmd::agent_param_name(text) {
            style.agent = name;
        }
        // Macro path: count how the file's upper-case macro calls are written.
        let (mut bare, mut short, mut long) = (0usize, 0usize, 0usize);
        let mut i = 0;
        while i < text.len() {
            if let Some(name) = crate::acmd_src::bare_macro_name_at(text, i, text.len()) {
                bare += 1;
                i = name.end;
                continue;
            }
            if text.is_char_boundary(i) && text[i..].starts_with("macros::") {
                let after = &text[i + "macros::".len()..];
                if after.starts_with(|c: char| c.is_ascii_uppercase()) {
                    if text[..i].ends_with("smash_script::") {
                        long += 1;
                    } else {
                        short += 1;
                    }
                }
                i += "macros::".len();
                continue;
            }
            i += 1;
        }
        style.macro_path = if bare > short + long {
            String::new()
        } else if long > short {
            "smash_script::macros::".into()
        } else {
            "macros::".into()
        };
        // `is_excute`: as the first one is written.
        style.excute_path = text
            .find("is_excute(")
            .map(|at| {
                let line_start = text[..at].rfind('\n').map_or(0, |n| n + 1);
                let lead = &text[line_start..at];
                let path_start = lead
                    .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':'))
                    .map_or(0, |n| n + 1);
                lead[path_start..].to_string()
            })
            .unwrap_or_else(|| style.macro_path.clone());
        // `frame(X, N)`: X and whether N carries a decimal point.
        if let Some(at) = text.find("frame(") {
            let args = &text[at + "frame(".len()..];
            if let Some(comma) = args.find(',') {
                let state = args[..comma].trim();
                if !state.is_empty() && !state.contains(['(', ')', '\n']) {
                    style.lua_state = state.to_string();
                }
                let number = args[comma + 1..]
                    .split(')')
                    .next()
                    .unwrap_or("")
                    .trim();
                if number.parse::<f32>().is_ok() {
                    style.float_frames = number.contains('.');
                }
            }
        } else if style.agent != "agent" {
            style.lua_state = format!("{}.lua_state_agent", style.agent);
        }
        // Indentation: the step from an `if` line to the first line inside it.
        let lines: Vec<&str> = text.lines().collect();
        for pair in lines.windows(2) {
            let (outer, inner) = (pair[0], pair[1]);
            if !outer.trim_end().ends_with('{') || inner.trim().is_empty() {
                continue;
            }
            let outer_lead = &outer[..outer.len() - outer.trim_start().len()];
            let inner_lead = &inner[..inner.len() - inner.trim_start().len()];
            if let Some(step) = inner_lead.strip_prefix(outer_lead) {
                if !step.is_empty() && step.chars().all(|c| c == ' ' || c == '\t') {
                    style.indent = step.to_string();
                    break;
                }
            }
        }
        style
    }

    /// Whether `apply` would change nothing.
    #[cfg(test)]
    pub fn is_canonical(&self) -> bool {
        let canonical = SourceStyle {
            newline: self.newline,
            ..SourceStyle::default()
        };
        *self == canonical && self.newline == "\n"
    }

    /// Canonical generated `text` re-spelled in this style.
    ///
    /// Leading indentation is converted only where it is made of canonical four-space steps
    /// beyond `base` — the indentation the caller copied from the file, which is already right.
    pub fn apply(&self, text: &str, base: &str) -> String {
        let mut out = String::with_capacity(text.len() + 16);
        for (index, raw_line) in text.split('\n').enumerate() {
            if index > 0 {
                out.push_str(self.newline);
            }
            let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            let (lead, body) = line.split_at(line.len() - line.trim_start().len());
            out.push_str(&self.reindent(lead, base));
            out.push_str(&self.respell(body));
        }
        out
    }

    fn reindent(&self, lead: &str, base: &str) -> String {
        if self.indent == "    " {
            return lead.to_string();
        }
        let Some(extra) = lead.strip_prefix(base) else {
            return lead.to_string();
        };
        if extra.is_empty() || !extra.chars().all(|c| c == ' ') {
            return lead.to_string();
        }
        let steps = extra.len() / 4;
        let rest = extra.len() % 4;
        format!("{base}{}{}", self.indent.repeat(steps), " ".repeat(rest))
    }

    fn respell(&self, body: &str) -> String {
        let mut text = body.to_string();
        if self.lua_state != "agent.lua_state_agent" {
            text = text.replace("agent.lua_state_agent", &self.lua_state);
        }
        if !self.float_frames {
            text = integer_frames(&text);
        }
        if self.excute_path != "macros::" {
            text = text.replace("macros::is_excute(", &format!("{}is_excute(", self.excute_path));
        }
        if self.macro_path != "macros::" {
            text = replace_macro_path(&text, &self.macro_path);
        }
        if self.agent != "agent" {
            text = rename_identifier(&text, "agent", &self.agent);
        }
        text
    }
}

/// `frame(x, 5.0)` → `frame(x, 5)`, and the same for `wait`.
fn integer_frames(text: &str) -> String {
    let mut out = text.to_string();
    for head in ["frame(", "wait("] {
        let mut search = 0;
        while let Some(rel) = out[search..].find(head) {
            let at = search + rel + head.len();
            search = at;
            let Some(comma) = out[at..].find(',').map(|n| at + n) else { break };
            let Some(close) = out[comma..].find(')').map(|n| comma + n) else { break };
            let number = out[comma + 1..close].trim();
            if let Some(whole) = number.strip_suffix(".0") {
                if whole.parse::<i64>().is_ok() {
                    let replacement = format!(" {whole}");
                    out.replace_range(comma + 1..close, &replacement);
                }
            }
        }
    }
    out
}

/// `macros::NAME(` → `{path}NAME(` for upper-case macro names only.
fn replace_macro_path(text: &str, path: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("macros::") {
        let after = &rest[at + "macros::".len()..];
        let qualified = rest[..at].ends_with("::");
        out.push_str(&rest[..at]);
        if !qualified && after.starts_with(|c: char| c.is_ascii_uppercase()) {
            out.push_str(path);
        } else {
            out.push_str("macros::");
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Every whole-word `from` outside string literals renamed to `to`.
fn rename_identifier(text: &str, from: &str, to: &str) -> String {
    let bytes = text.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_string = false;
    while i < text.len() {
        let b = bytes[i];
        if b == b'"' && (i == 0 || bytes[i - 1] != b'\\') {
            in_string = !in_string;
        }
        if !in_string
            && text[i..].starts_with(from)
            && (i == 0 || !is_ident(bytes[i - 1]))
            && bytes.get(i + from.len()).is_none_or(|n| !is_ident(*n))
            && (i == 0 || bytes[i - 1] != b'.')
        {
            out.push_str(to);
            i += from.len();
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

    const CANONICAL: &str = "    frame(agent.lua_state_agent, 12.0);\n    if macros::is_excute(agent) {\n        macros::EFFECT(agent, Hash40::new(\"sys_agent\"), Hash40::new(\"top\"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, true);\n        AttackModule::clear_all(agent.module_accessor);\n    }\n";

    #[test]
    fn a_canonical_file_is_left_exactly_as_generated() {
        let file = "unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {\n    frame(agent.lua_state_agent, 3.0);\n    if macros::is_excute(agent) {\n        macros::ATTACK(agent);\n    }\n}\n";
        let style = SourceStyle::detect(file);
        assert!(style.is_canonical(), "{style:?}");
        assert_eq!(style.apply(CANONICAL, "    "), CANONICAL);
    }

    #[test]
    fn a_bare_tabbed_crlf_fighter_file_gets_lines_in_its_own_style() {
        let file = "unsafe extern \"C\" fn game_x(fighter: &mut L2CAgentBase) {\r\n\tframe(fighter.lua_state_agent, 3);\r\n\tif is_excute(fighter) {\r\n\t\tATTACK(fighter, 0);\r\n\t\tEFFECT(fighter, 0);\r\n\t}\r\n}\r\n";
        let style = SourceStyle::detect(file);
        assert_eq!(style.agent, "fighter");
        assert_eq!(style.macro_path, "");
        assert_eq!(style.excute_path, "");
        assert_eq!(style.lua_state, "fighter.lua_state_agent");
        assert!(!style.float_frames);
        assert_eq!(style.indent, "\t");
        assert_eq!(style.newline, "\r\n");
        let canonical = "\tframe(agent.lua_state_agent, 12.0);\n\tif macros::is_excute(agent) {\n\t    macros::EFFECT(agent, Hash40::new(\"sys_agent\"));\n\t    AttackModule::clear_all(agent.module_accessor);\n\t}\n";
        assert_eq!(
            style.apply(canonical, "\t"),
            "\tframe(fighter.lua_state_agent, 12);\r\n\tif is_excute(fighter) {\r\n\t\tEFFECT(fighter, Hash40::new(\"sys_agent\"));\r\n\t\tAttackModule::clear_all(fighter.module_accessor);\r\n\t}\r\n"
        );
    }

    #[test]
    fn smash_script_qualified_files_keep_the_long_path() {
        let file = "fn effect_x(agent: &mut L2CAgentBase) {\n    if smash_script::macros::is_excute(agent) {\n        smash_script::macros::EFFECT(agent);\n        smash_script::macros::LAST_EFFECT_SET_RATE(agent, 1.0);\n    }\n}\n";
        let style = SourceStyle::detect(file);
        assert_eq!(style.macro_path, "smash_script::macros::");
        assert_eq!(
            style.apply("    if macros::is_excute(agent) {\n        macros::EFFECT(agent);\n    }", "    "),
            "    if smash_script::macros::is_excute(agent) {\n        smash_script::macros::EFFECT(agent);\n    }"
        );
    }
}
