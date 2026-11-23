//! The scale a moveset sets on its fighter every frame, for the viewport.
//!
//! A moveset that resizes its fighter does it from the fighter's frame callback (`on_line`),
//! usually through a helper of its own. Shy Guy's:
//!
//! ```text
//! pub unsafe fn set_model_scale(module_accessor: *mut BattleObjectModuleAccessor, size: f32) {
//!     ModelModule::set_scale(module_accessor, size);
//!     AttackModule::set_attack_scale(module_accessor, size, true);
//!     GrabModule::set_size_mul(module_accessor, size);
//! }
//! // in shyguy_frame, registered with `agent.on_line(Main, shyguy_frame)`:
//! if !fighter.is_status_one_of(&[*FIGHTER_STATUS_KIND_WIN, *FIGHTER_STATUS_KIND_LOSE, …]) {
//!     set_model_scale(fighter.module_accessor, MODEL_SCALE);
//! } else {
//!     set_model_scale(fighter.module_accessor, 1.0);
//! }
//! ```
//!
//! The three calls are read separately, because they do different things and a helper can leave
//! any of them out: [`ModelModule::set_scale`] resizes the model and skeleton, `set_attack_scale`
//! the hitboxes, and `GrabModule::set_size_mul` the grab boxes.
//!
//! The frame callback is walked the way it runs during an ordinary move: a branch that only runs
//! on the results screen or while entering (`WIN`, `LOSE`, `ENTRY`, …) is not taken, and the
//! common `ModelModule::scale(…) == get_param_float(…"scale"…)` guard is taken. A condition this
//! reader can't decide is assumed true, so an unknown `if` runs its body and skips its `else`.
//! Helpers are followed into their bodies with their arguments bound, so a helper that skips
//! `GrabModule` leaves the grab scale alone.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use syn::spanned::Spanned;

/// Statuses outside ordinary play. A frame callback branch that only runs in one of these does
/// not decide the scale a move is seen at.
const NON_GAMEPLAY_STATUSES: &[&str] = &[
    "FIGHTER_STATUS_KIND_WIN",
    "FIGHTER_STATUS_KIND_LOSE",
    "FIGHTER_STATUS_KIND_NONE",
    "FIGHTER_STATUS_KIND_ENTRY",
    "FIGHTER_STATUS_KIND_DEMO",
    "FIGHTER_STATUS_KIND_STANDBY",
    "FIGHTER_STATUS_KIND_DEAD",
    "FIGHTER_STATUS_KIND_REBIRTH",
];

/// How deep helper calls are followed.
const MAX_DEPTH: usize = 8;

/// One scale call the frame callback makes.
#[derive(Debug, Clone, PartialEq)]
pub struct ScaleCall {
    pub value: f32,
    pub file: PathBuf,
    /// 1-based line of the call.
    pub line: usize,
}

/// The scales one fighter's frame callback sets. `None` for a call it never makes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceScales {
    /// `ModelModule::set_scale`.
    pub model: Option<ScaleCall>,
    /// `AttackModule::set_attack_scale`.
    pub attack: Option<ScaleCall>,
    /// `GrabModule::set_size_mul`.
    pub grab: Option<ScaleCall>,
}

impl SourceScales {
    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.attack.is_none() && self.grab.is_none()
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Model,
    Attack,
    Grab,
}

/// Every fighter's frame-callback scales in `sources`, by normalized fighter name.
/// `fighter_of` names the fighter a file belongs to when a registration's agent is a parameter.
pub fn scan(
    sources: &[(PathBuf, String)],
    fighter_of: impl Fn(&Path) -> Option<String>,
    consts: &crate::project_consts::ProjectConsts,
) -> HashMap<String, SourceScales> {
    let parsed: Vec<(&Path, syn::File)> = sources
        .iter()
        .filter_map(|(file, text)| Some((file.as_path(), syn::parse_file(text).ok()?)))
        .collect();
    let mut functions: HashMap<String, Vec<(&Path, &syn::ItemFn)>> = HashMap::new();
    for (file, ast) in &parsed {
        collect_fns(&ast.items, &mut |item| {
            functions
                .entry(item.sig.ident.to_string())
                .or_default()
                .push((*file, item));
        });
    }

    let mut callbacks: Vec<(String, String, &Path)> = Vec::new();
    for (file, ast) in &parsed {
        collect_fns(&ast.items, &mut |item| {
            // smashline 1: `#[fighter_frame(agent = FIGHTER_KIND_PACMAN)]`.
            if let Some(fighter) = frame_attribute_fighter(item) {
                callbacks.push((fighter, item.sig.ident.to_string(), *file));
            }
            let mut bindings: HashMap<String, String> = HashMap::new();
            find_on_line(&item.block, &mut bindings, &mut |agent, callback| {
                if let Some(fighter) = agent.or_else(|| fighter_of(file)) {
                    callbacks.push((fighter, callback, *file));
                }
            });
        });
    }

    let mut result: HashMap<String, SourceScales> = HashMap::new();
    for (fighter, callback, file) in callbacks {
        let Some((file, item)) = pick_fn(&functions, &callback, file) else {
            continue;
        };
        let mut walker = Walker {
            functions: &functions,
            consts,
            found: SourceScales::default(),
            stack: HashSet::new(),
        };
        walker.walk_fn(file, item, Vec::new(), 0);
        if walker.found.is_empty() {
            continue;
        }
        let entry = result.entry(normalize_fighter(&fighter)).or_default();
        // The first callback to set a scale wins, as the first call of a frame would be the one
        // a later call of the same frame overrides; there is rarely more than one.
        entry.model = entry.model.take().or(walker.found.model);
        entry.attack = entry.attack.take().or(walker.found.attack);
        entry.grab = entry.grab.take().or(walker.found.grab);
    }
    result
}

/// A `fighter_param` `scale` the moveset overrides through `param_config`:
/// `update_float_2(-(*FIGHTER_KIND_PACMAN), …, (hash40("scale"), 0, 0.8))`.
pub fn param_config_scale<'a>(sources: impl IntoIterator<Item = &'a str>) -> Option<f32> {
    let mut found: Option<f32> = None;
    for text in sources {
        for line in text.lines() {
            let code = line.split("//").next().unwrap_or("");
            let Some(at) = code.find("hash40(\"scale\")") else {
                continue;
            };
            if !code.contains("update_float") {
                continue;
            }
            let rest = &code[at + "hash40(\"scale\")".len()..];
            let tuple = rest.split(')').next().unwrap_or("");
            let Some(value) = tuple
                .split(',')
                .filter_map(|part| part.trim().trim_end_matches("f32").parse::<f32>().ok())
                .next_back()
            else {
                continue;
            };
            match found {
                None => found = Some(value),
                Some(known) if (known - value).abs() > f32::EPSILON => return None,
                _ => {}
            }
        }
    }
    found
}

fn normalize_fighter(name: &str) -> String {
    let name = name.trim().to_ascii_lowercase();
    name.strip_prefix("fighter_").unwrap_or(&name).to_string()
}

fn collect_fns<'a>(items: &'a [syn::Item], visit: &mut impl FnMut(&'a syn::ItemFn)) {
    for item in items {
        match item {
            syn::Item::Fn(function) => visit(function),
            syn::Item::Mod(module) => {
                if let Some((_, items)) = &module.content {
                    collect_fns(items, visit);
                }
            }
            _ => {}
        }
    }
}

/// The definition of `name` that a call from `file` reaches: one in the same file first.
fn pick_fn<'a>(
    functions: &HashMap<String, Vec<(&'a Path, &'a syn::ItemFn)>>,
    name: &str,
    file: &Path,
) -> Option<(&'a Path, &'a syn::ItemFn)> {
    let candidates = functions.get(name)?;
    candidates
        .iter()
        .find(|(f, _)| *f == file)
        .or_else(|| (candidates.len() == 1).then(|| &candidates[0]))
        .copied()
}

fn frame_attribute_fighter(item: &syn::ItemFn) -> Option<String> {
    item.attrs.iter().find_map(|attr| {
        let path = attr.path().segments.last()?.ident.to_string();
        if path != "fighter_frame" {
            return None;
        }
        let tokens = attr.meta.require_list().ok()?.tokens.to_string();
        let kind = tokens.split("FIGHTER_KIND_").nth(1)?;
        let kind: String = kind
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        Some(kind.to_ascii_lowercase())
    })
}

/// The last path segment of `expr` when it is a path (`Main`, `smashline::Main`, `my_frame`).
fn path_name(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Path(path) => Some(path.path.segments.last()?.ident.to_string()),
        syn::Expr::Reference(reference) => path_name(&reference.expr),
        syn::Expr::Paren(paren) => path_name(&paren.expr),
        _ => None,
    }
}

/// The fighter of an `Agent::new("…")` expression.
fn agent_new(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Call(call) => {
            let syn::Expr::Path(func) = &*call.func else {
                return None;
            };
            let segments: Vec<String> = func
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            if !segments.ends_with(&["Agent".to_string(), "new".to_string()]) {
                return None;
            }
            match call.args.first()? {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(name),
                    ..
                }) => Some(name.value()),
                _ => None,
            }
        }
        syn::Expr::Reference(reference) => agent_new(&reference.expr),
        syn::Expr::Paren(paren) => agent_new(&paren.expr),
        _ => None,
    }
}

/// The agent an `.on_line(…)` chain starts from: `Agent::new("…")` itself or a `let` of one.
fn chain_agent(expr: &syn::Expr, bindings: &HashMap<String, String>) -> Option<String> {
    match expr {
        syn::Expr::MethodCall(call) => chain_agent(&call.receiver, bindings),
        syn::Expr::Path(path) if path.path.segments.len() == 1 => bindings
            .get(&path.path.segments[0].ident.to_string())
            .cloned(),
        syn::Expr::Reference(reference) => chain_agent(&reference.expr, bindings),
        syn::Expr::Paren(paren) => chain_agent(&paren.expr, bindings),
        _ => agent_new(expr),
    }
}

/// Every `.on_line(…, callback)` in `block`, with the agent when the chain names one.
fn find_on_line(
    block: &syn::Block,
    bindings: &mut HashMap<String, String>,
    found: &mut impl FnMut(Option<String>, String),
) {
    for stmt in &block.stmts {
        match stmt {
            syn::Stmt::Local(local) => {
                if let (syn::Pat::Ident(name), Some(init)) = (&local.pat, &local.init) {
                    if let Some(fighter) = agent_new(&init.expr) {
                        bindings.insert(name.ident.to_string(), fighter);
                    }
                }
                if let Some(init) = &local.init {
                    find_on_line_expr(&init.expr, bindings, found);
                }
            }
            syn::Stmt::Expr(expr, _) => find_on_line_expr(expr, bindings, found),
            _ => {}
        }
    }
}

fn find_on_line_expr(
    expr: &syn::Expr,
    bindings: &mut HashMap<String, String>,
    found: &mut impl FnMut(Option<String>, String),
) {
    match expr {
        syn::Expr::MethodCall(call) => {
            if call.method == "on_line" && call.args.len() >= 2 {
                if let Some(callback) = path_name(&call.args[1]) {
                    found(chain_agent(&call.receiver, bindings), callback);
                }
            }
            find_on_line_expr(&call.receiver, bindings, found);
        }
        syn::Expr::Block(block) => find_on_line(&block.block, bindings, found),
        syn::Expr::Unsafe(block) => find_on_line(&block.block, bindings, found),
        syn::Expr::If(expr_if) => {
            find_on_line(&expr_if.then_branch, bindings, found);
            if let Some((_, other)) = &expr_if.else_branch {
                find_on_line_expr(other, bindings, found);
            }
        }
        _ => {}
    }
}

/// A value the walker can know: a number, or nothing.
type Env = Vec<(String, Option<f32>)>;

struct Walker<'a> {
    functions: &'a HashMap<String, Vec<(&'a Path, &'a syn::ItemFn)>>,
    consts: &'a crate::project_consts::ProjectConsts,
    found: SourceScales,
    /// Functions being walked, so a recursive helper ends.
    stack: HashSet<String>,
}

/// Whether a walk ran to the end of a block or hit a `return`.
#[derive(PartialEq)]
enum Flow {
    Next,
    Return,
}

impl<'a> Walker<'a> {
    fn walk_fn(
        &mut self,
        file: &'a Path,
        item: &'a syn::ItemFn,
        args: Vec<Option<f32>>,
        depth: usize,
    ) {
        let name = item.sig.ident.to_string();
        if depth > MAX_DEPTH || !self.stack.insert(name.clone()) {
            return;
        }
        let mut env: Env = Vec::new();
        for (index, input) in item.sig.inputs.iter().enumerate() {
            if let syn::FnArg::Typed(typed) = input {
                if let syn::Pat::Ident(param) = &*typed.pat {
                    env.push((param.ident.to_string(), args.get(index).copied().flatten()));
                }
            }
        }
        self.walk_block(file, &item.block, &mut env, depth);
        self.stack.remove(&name);
    }

    fn walk_block(
        &mut self,
        file: &'a Path,
        block: &'a syn::Block,
        env: &mut Env,
        depth: usize,
    ) -> Flow {
        let scope = env.len();
        for stmt in &block.stmts {
            let flow = match stmt {
                syn::Stmt::Local(local) => {
                    let value = local
                        .init
                        .as_ref()
                        .and_then(|init| self.value(&init.expr, env));
                    if let Some(init) = &local.init {
                        if self.walk_expr(file, &init.expr, env, depth) == Flow::Return {
                            env.truncate(scope);
                            return Flow::Return;
                        }
                    }
                    if let syn::Pat::Ident(name) = &local.pat {
                        env.push((name.ident.to_string(), value));
                    } else if let syn::Pat::Type(typed) = &local.pat {
                        if let syn::Pat::Ident(name) = &*typed.pat {
                            env.push((name.ident.to_string(), value));
                        }
                    }
                    Flow::Next
                }
                syn::Stmt::Expr(expr, _) => self.walk_expr(file, expr, env, depth),
                _ => Flow::Next,
            };
            if flow == Flow::Return {
                env.truncate(scope);
                return Flow::Return;
            }
        }
        env.truncate(scope);
        Flow::Next
    }

    fn walk_expr(
        &mut self,
        file: &'a Path,
        expr: &'a syn::Expr,
        env: &mut Env,
        depth: usize,
    ) -> Flow {
        match expr {
            syn::Expr::Return(_) => Flow::Return,
            syn::Expr::Block(block) => self.walk_block(file, &block.block, env, depth),
            syn::Expr::Unsafe(block) => self.walk_block(file, &block.block, env, depth),
            syn::Expr::Paren(paren) => self.walk_expr(file, &paren.expr, env, depth),
            syn::Expr::If(expr_if) => match Self::condition(&expr_if.cond) {
                Some(false) => match &expr_if.else_branch {
                    Some((_, other)) => self.walk_expr(file, other, env, depth),
                    None => Flow::Next,
                },
                _ => self.walk_block(file, &expr_if.then_branch, env, depth),
            },
            syn::Expr::Call(call) => {
                for arg in &call.args {
                    self.walk_expr(file, arg, env, depth);
                }
                let syn::Expr::Path(func) = &*call.func else {
                    return Flow::Next;
                };
                let segments: Vec<String> = func
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect();
                let tail: Vec<&str> = segments
                    .iter()
                    .rev()
                    .take(2)
                    .rev()
                    .map(String::as_str)
                    .collect();
                let kind = match tail.as_slice() {
                    ["ModelModule", "set_scale"] => Some(Kind::Model),
                    ["AttackModule", "set_attack_scale"] => Some(Kind::Attack),
                    ["GrabModule", "set_size_mul"] => Some(Kind::Grab),
                    _ => None,
                };
                if let Some(kind) = kind {
                    if let Some(value) =
                        call.args.iter().nth(1).and_then(|arg| self.value(arg, env))
                    {
                        self.record(kind, value, file, call.span().start().line);
                    }
                    return Flow::Next;
                }
                let Some(name) = segments.last() else {
                    return Flow::Next;
                };
                if let Some((callee_file, callee)) = pick_fn(self.functions, name, file) {
                    let args = call.args.iter().map(|arg| self.value(arg, env)).collect();
                    self.walk_fn(callee_file, callee, args, depth + 1);
                }
                Flow::Next
            }
            syn::Expr::MethodCall(call) => {
                self.walk_expr(file, &call.receiver, env, depth);
                for arg in &call.args {
                    self.walk_expr(file, arg, env, depth);
                }
                Flow::Next
            }
            syn::Expr::Match(expr_match) => {
                // Which arm runs is rarely knowable here; the first arm that sets a scale stands
                // for the match.
                for arm in &expr_match.arms {
                    if self.walk_expr(file, &arm.body, env, depth) == Flow::Return {
                        return Flow::Return;
                    }
                }
                Flow::Next
            }
            syn::Expr::Loop(expr_loop) => {
                self.walk_block(file, &expr_loop.body, env, depth);
                Flow::Next
            }
            syn::Expr::While(expr_while) => {
                self.walk_block(file, &expr_while.body, env, depth);
                Flow::Next
            }
            syn::Expr::ForLoop(expr_for) => {
                self.walk_block(file, &expr_for.body, env, depth);
                Flow::Next
            }
            _ => Flow::Next,
        }
    }

    fn record(&mut self, kind: Kind, value: f32, file: &Path, line: usize) {
        let slot = match kind {
            Kind::Model => &mut self.found.model,
            Kind::Attack => &mut self.found.attack,
            Kind::Grab => &mut self.found.grab,
        };
        if slot.is_none() {
            *slot = Some(ScaleCall {
                value,
                file: file.to_path_buf(),
                line,
            });
        }
    }

    /// A number `expr` evaluates to: a literal, a local, a bound parameter, a project const, or
    /// arithmetic on those.
    fn value(&self, expr: &syn::Expr, env: &Env) -> Option<f32> {
        match expr {
            syn::Expr::Lit(lit) => match &lit.lit {
                syn::Lit::Float(f) => f.base10_parse::<f32>().ok(),
                syn::Lit::Int(i) => i.base10_parse::<f32>().ok(),
                _ => None,
            },
            syn::Expr::Paren(paren) => self.value(&paren.expr, env),
            syn::Expr::Group(group) => self.value(&group.expr, env),
            syn::Expr::Cast(cast) => self.value(&cast.expr, env),
            syn::Expr::Unary(syn::ExprUnary {
                op: syn::UnOp::Neg(_),
                expr,
                ..
            }) => self.value(expr, env).map(|v| -v),
            syn::Expr::Binary(binary) => {
                let (left, right) = (
                    self.value(&binary.left, env)?,
                    self.value(&binary.right, env)?,
                );
                match binary.op {
                    syn::BinOp::Add(_) => Some(left + right),
                    syn::BinOp::Sub(_) => Some(left - right),
                    syn::BinOp::Mul(_) => Some(left * right),
                    syn::BinOp::Div(_) if right != 0.0 => Some(left / right),
                    _ => None,
                }
            }
            syn::Expr::Path(path) => {
                let name = path.path.segments.last()?.ident.to_string();
                if path.path.segments.len() == 1 {
                    if let Some((_, value)) = env.iter().rev().find(|(n, _)| *n == name) {
                        return *value;
                    }
                }
                self.consts.get(&name)?.number().map(|v| v as f32)
            }
            _ => None,
        }
    }

    /// Whether `cond` holds during an ordinary move; `None` when this reader can't tell.
    fn condition(cond: &syn::Expr) -> Option<bool> {
        match cond {
            syn::Expr::Paren(paren) => Self::condition(&paren.expr),
            syn::Expr::Unary(syn::ExprUnary {
                op: syn::UnOp::Not(_),
                expr,
                ..
            }) => Self::condition(expr).map(|v| !v),
            syn::Expr::Binary(binary) => match binary.op {
                syn::BinOp::And(_) => match (
                    Self::condition(&binary.left),
                    Self::condition(&binary.right),
                ) {
                    (Some(false), _) | (_, Some(false)) => Some(false),
                    (Some(true), Some(true)) => Some(true),
                    _ => None,
                },
                syn::BinOp::Or(_) => match (
                    Self::condition(&binary.left),
                    Self::condition(&binary.right),
                ) {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (Some(false), Some(false)) => Some(false),
                    _ => None,
                },
                syn::BinOp::Eq(_) | syn::BinOp::Ne(_) => {
                    let equal = matches!(binary.op, syn::BinOp::Eq(_));
                    let (left, right) = (words(&binary.left), words(&binary.right));
                    // The guard most scale helpers share: still at the param's own scale.
                    let reads_scale = |w: &[String]| {
                        w.iter().any(|w| w == "ModelModule") && w.iter().any(|w| w == "scale")
                    };
                    let param_scale = |w: &[String]| {
                        w.iter().any(|w| w == "get_param_float")
                            && w.iter().any(|w| w == "\"scale\"")
                    };
                    if (reads_scale(&left) && param_scale(&right))
                        || (reads_scale(&right) && param_scale(&left))
                    {
                        return Some(equal);
                    }
                    if only_non_gameplay_statuses(&left) || only_non_gameplay_statuses(&right) {
                        return Some(!equal);
                    }
                    None
                }
                _ => None,
            },
            syn::Expr::MethodCall(call)
                if call.method == "is_status_one_of" || call.method == "is_status" =>
            {
                only_non_gameplay_statuses(&words(cond)).then_some(false)
            }
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Bool(b),
                ..
            }) => Some(b.value),
            _ => None,
        }
    }
}

/// Path segments and string literals anywhere in `expr`, in order: enough to recognise a status
/// constant or the `ModelModule::scale` / `get_param_float(…"scale"…)` guard.
fn words(expr: &syn::Expr) -> Vec<String> {
    fn walk(expr: &syn::Expr, out: &mut Vec<String>) {
        match expr {
            syn::Expr::Path(path) => {
                out.extend(path.path.segments.iter().map(|s| s.ident.to_string()));
            }
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(text),
                ..
            }) => out.push(format!("\"{}\"", text.value())),
            syn::Expr::Call(call) => {
                walk(&call.func, out);
                call.args.iter().for_each(|arg| walk(arg, out));
            }
            syn::Expr::MethodCall(call) => {
                walk(&call.receiver, out);
                out.push(call.method.to_string());
                call.args.iter().for_each(|arg| walk(arg, out));
            }
            syn::Expr::Macro(mac) => {
                // `hash40!("scale")`-style shorthands and `vec![…]` keep their tokens' words.
                for token in mac.mac.tokens.clone() {
                    match token {
                        proc_macro2::TokenTree::Ident(ident) => out.push(ident.to_string()),
                        proc_macro2::TokenTree::Literal(lit) => out.push(lit.to_string()),
                        _ => {}
                    }
                }
            }
            syn::Expr::Array(array) => array.elems.iter().for_each(|e| walk(e, out)),
            syn::Expr::Reference(reference) => walk(&reference.expr, out),
            syn::Expr::Unary(unary) => walk(&unary.expr, out),
            syn::Expr::Paren(paren) => walk(&paren.expr, out),
            syn::Expr::Group(group) => walk(&group.expr, out),
            syn::Expr::Cast(cast) => walk(&cast.expr, out),
            syn::Expr::Field(field) => walk(&field.base, out),
            syn::Expr::Binary(binary) => {
                walk(&binary.left, out);
                walk(&binary.right, out);
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(expr, &mut out);
    out
}

/// Whether `words` name at least one status and only statuses outside ordinary play.
fn only_non_gameplay_statuses(words: &[String]) -> bool {
    let statuses: Vec<&String> = words
        .iter()
        .filter(|w| w.starts_with("FIGHTER_STATUS_KIND_"))
        .collect();
    !statuses.is_empty()
        && statuses
            .iter()
            .all(|status| NON_GAMEPLAY_STATUSES.contains(&status.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_texts(files: &[(&str, &str)], fighter: Option<&str>) -> HashMap<String, SourceScales> {
        let sources: Vec<(PathBuf, String)> = files
            .iter()
            .map(|(path, text)| (PathBuf::from(path), text.to_string()))
            .collect();
        let consts = crate::project_consts::ProjectConsts::scan(&sources);
        scan(&sources, |_| fighter.map(str::to_string), &consts)
    }

    const SHYGUY_UTIL: &str = r#"
pub unsafe fn set_model_scale(module_accessor: *mut BattleObjectModuleAccessor, size: f32) {
    ModelModule::set_scale(module_accessor, size);
    AttackModule::set_attack_scale(module_accessor, size, true);
    GrabModule::set_size_mul(module_accessor, size);
}
"#;

    #[test]
    fn a_helper_called_from_the_frame_callback_sets_all_three() {
        let frame = r#"
pub const MODEL_SCALE: f32 = 0.80;
unsafe extern "C" fn shyguy_frame(fighter: &mut L2CFighterCommon) {
    if !fighter.is_status_one_of(&[*FIGHTER_STATUS_KIND_WIN, *FIGHTER_STATUS_KIND_LOSE, *FIGHTER_STATUS_KIND_NONE]) {
        set_model_scale(fighter.module_accessor, MODEL_SCALE);
    }
    else {
        set_model_scale(fighter.module_accessor, 1.0);
    }
}
pub(super) fn install(agent: &mut Agent) {
    agent.on_line(Main, shyguy_frame).on_start(on_start);
}
"#;
        let found = scan_texts(
            &[("util.rs", SHYGUY_UTIL), ("frame.rs", frame)],
            Some("pacman"),
        );
        let scales = &found["pacman"];
        assert_eq!(scales.model.as_ref().map(|c| c.value), Some(0.8));
        assert_eq!(scales.attack.as_ref().map(|c| c.value), Some(0.8));
        assert_eq!(scales.grab.as_ref().map(|c| c.value), Some(0.8));
        assert_eq!(
            scales.model.as_ref().unwrap().file,
            PathBuf::from("util.rs")
        );
    }

    #[test]
    fn a_helper_that_leaves_out_grab_module_leaves_the_grab_scale_alone() {
        let util = r#"
pub unsafe fn resize(boma: *mut BattleObjectModuleAccessor, size: f32) {
    ModelModule::set_scale(boma, size);
    AttackModule::set_attack_scale(boma, size, true);
}
"#;
        let frame = r#"
unsafe extern "C" fn frame(fighter: &mut L2CFighterCommon) {
    resize(fighter.module_accessor, 1.18);
}
pub fn install() {
    Agent::new("ganon").on_line(Main, frame).install();
}
"#;
        let found = scan_texts(&[("util.rs", util), ("frame.rs", frame)], None);
        let scales = &found["ganon"];
        assert_eq!(scales.model.as_ref().map(|c| c.value), Some(1.18));
        assert_eq!(scales.attack.as_ref().map(|c| c.value), Some(1.18));
        assert_eq!(scales.grab, None);
    }

    #[test]
    fn direct_calls_with_different_values_and_the_param_guard() {
        let frame = r#"
unsafe extern "C" fn scale(fighter : &mut L2CFighterCommon) {
    unsafe {
        let boma = smash::app::sv_system::battle_object_module_accessor(fighter.lua_state_agent);
        if ModelModule::scale(boma) == WorkModule::get_param_float(fighter.module_accessor, hash40("scale"), 0) {
            ModelModule::set_scale(boma, 0.94);
            AttackModule::set_attack_scale(boma, 0.95, true);
            GrabModule::set_size_mul(boma, 0.94);
        }
    }
}
pub fn install() {
    let agent = &mut smashline::Agent::new("fighter_wario");
    agent.on_line(smashline::Main, scale);
    agent.install();
}
"#;
        let found = scan_texts(&[("mod.rs", frame)], None);
        let scales = &found["wario"];
        assert_eq!(scales.model.as_ref().map(|c| c.value), Some(0.94));
        assert_eq!(scales.attack.as_ref().map(|c| c.value), Some(0.95));
        assert_eq!(scales.grab.as_ref().map(|c| c.value), Some(0.94));
    }

    #[test]
    fn a_local_size_and_an_early_return_on_the_results_screen() {
        let frame = r#"
unsafe extern "C" fn frame(fighter: &mut L2CFighterCommon) {
    if StatusModule::status_kind(fighter.module_accessor) == *FIGHTER_STATUS_KIND_WIN {
        return;
    }
    let size = 1.1 * 1.0;
    ModelModule::set_scale(fighter.module_accessor, size);
}
fn install(agent: &mut Agent) { agent.on_line(Main, frame); }
"#;
        let found = scan_texts(&[("frame.rs", frame)], Some("mario"));
        assert_eq!(found["mario"].model.as_ref().map(|c| c.value), Some(1.1));
        assert_eq!(found["mario"].model.as_ref().map(|c| c.line), Some(7));
    }

    #[test]
    fn a_frame_callback_that_returns_on_ordinary_play_sets_nothing() {
        let frame = r#"
unsafe extern "C" fn frame(fighter: &mut L2CFighterCommon) {
    return;
    ModelModule::set_scale(fighter.module_accessor, 2.0);
}
fn install(agent: &mut Agent) { agent.on_line(Main, frame); }
"#;
        assert!(scan_texts(&[("frame.rs", frame)], Some("mario")).is_empty());
    }

    #[test]
    fn param_config_override() {
        let text = "param_config::update_float_2(-(*FIGHTER_KIND_PACMAN), slots.clone(), (hash40(\"scale\"), 0, 0.8));";
        assert_eq!(param_config_scale([text]), Some(0.8));
        assert_eq!(
            param_config_scale(["// update_float_2(x, (hash40(\"scale\"), 0, 0.8));"]),
            None
        );
    }
}
