//! Runtime hurtbox overrides a moveset installs from its fighter frame/init callbacks, for the
//! viewport.
//!
//! Movesets that reshape a base fighter's hurtboxes (for instance, Shy Guy's elongated torso
//! hurtbox on Pac-Man's `Waist` bone, and zero-radius disabled leg hurtboxes) call
//! `FighterUtil::set_hit_data` from their frame or entry callback — often through a helper like
//! `set_hurtboxes(fighter.module_accessor, &[Hurtbox::capsule(...), ...])` guarded by
//! `if status_kind == *FIGHTER_STATUS_KIND_ENTRY || is_reset()`.
//!
//! Unlike [`crate::fighter_scale`], which skips `FIGHTER_STATUS_KIND_ENTRY` branches because
//! scales are applied every gameplay frame, hurtbox setup intentionally runs on `ENTRY` / reset
//! and persists for the rest of the match, so this scanner walks `ENTRY` / reset branches while
//! skipping results-screen-only (`WIN` / `LOSE` / `DEMO`) branches.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use syn::spanned::Spanned;

/// Statuses that only run on victory/results/demo screens — never gameplay hurtbox setup.
const RESULTS_ONLY_STATUSES: &[&str] = &[
    "FIGHTER_STATUS_KIND_WIN",
    "FIGHTER_STATUS_KIND_LOSE",
    "FIGHTER_STATUS_KIND_DEMO",
];

/// How deep helper calls are followed.
const MAX_DEPTH: usize = 8;

/// One hurtbox capsule overridden by a moveset's fighter frame or initialization code.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceHurtbox {
    /// 0-based `hit_data` slot index passed to `FighterUtil::set_hit_data`.
    pub index: usize,
    /// Bone name as written in source (e.g. `"waist"`), matched case-insensitively against the
    /// loaded skeleton.
    pub bone_name: String,
    pub endpoint1: [f32; 3],
    pub endpoint2: [f32; 3],
    pub radius: f32,
    pub file: PathBuf,
    /// 1-based line of the call or entry.
    pub line: usize,
}

#[derive(Debug, Clone)]
struct ParsedHurtbox {
    bone_name: String,
    endpoint1: [f32; 3],
    endpoint2: [f32; 3],
    radius: f32,
    line: usize,
}

/// Every fighter's frame/init hurtbox overrides in `sources`, keyed by normalized fighter name.
/// `fighter_of` names the fighter a file belongs to when a registration's agent is a parameter.
pub fn scan(
    sources: &[(PathBuf, String)],
    fighter_of: impl Fn(&Path) -> Option<String>,
    consts: &crate::project_consts::ProjectConsts,
) -> HashMap<String, Vec<SourceHurtbox>> {
    let parsed: Vec<(&Path, syn::File)> = sources
        .iter()
        .filter_map(|(file, text)| Some((file.as_path(), syn::parse_file(text).ok()?)))
        .collect();

    let mut functions: HashMap<String, Vec<(&Path, &syn::ItemFn)>> = HashMap::new();
    let mut const_exprs: HashMap<String, &syn::Expr> = HashMap::new();
    for (file, ast) in &parsed {
        collect_items(
            &ast.items,
            &mut |item| {
                functions
                    .entry(item.sig.ident.to_string())
                    .or_default()
                    .push((*file, item));
            },
            &mut |name, expr| {
                const_exprs.entry(name).or_insert(expr);
            },
        );
    }

    let mut callbacks: Vec<(String, String, &Path)> = Vec::new();
    for (file, ast) in &parsed {
        collect_fns(&ast.items, &mut |item| {
            if let Some(fighter) = attribute_fighter(item) {
                callbacks.push((fighter, item.sig.ident.to_string(), *file));
            }
            let mut bindings: HashMap<String, String> = HashMap::new();
            find_callbacks(&item.block, &mut bindings, &mut |agent, callback| {
                if let Some(fighter) = agent.or_else(|| fighter_of(file)) {
                    callbacks.push((fighter, callback, *file));
                }
            });
        });
    }

    let mut result: HashMap<String, Vec<SourceHurtbox>> = HashMap::new();
    for (fighter, callback, file) in callbacks {
        let Some((file, item)) = pick_fn(&functions, &callback, file) else {
            continue;
        };
        let mut walker = Walker {
            functions: &functions,
            const_exprs: &const_exprs,
            consts,
            found: Vec::new(),
            stack: HashSet::new(),
        };
        walker.walk_fn(file, item, Vec::new(), 0);
        if walker.found.is_empty() {
            continue;
        }
        let entry = result.entry(normalize_fighter(&fighter)).or_default();
        for hurtbox in walker.found {
            if let Some(existing) = entry.iter_mut().find(|h| h.index == hurtbox.index) {
                *existing = hurtbox;
            } else {
                entry.push(hurtbox);
            }
        }
        entry.sort_by_key(|h| h.index);
    }
    result
}

fn normalize_fighter(name: &str) -> String {
    let name = name.trim().to_ascii_lowercase();
    name.strip_prefix("fighter_").unwrap_or(&name).to_string()
}

fn collect_items<'a>(
    items: &'a [syn::Item],
    visit_fn: &mut impl FnMut(&'a syn::ItemFn),
    visit_const: &mut impl FnMut(String, &'a syn::Expr),
) {
    for item in items {
        match item {
            syn::Item::Fn(function) => visit_fn(function),
            syn::Item::Const(c) => visit_const(c.ident.to_string(), &c.expr),
            syn::Item::Static(s) => visit_const(s.ident.to_string(), &s.expr),
            syn::Item::Mod(module) => {
                if let Some((_, items)) = &module.content {
                    collect_items(items, visit_fn, visit_const);
                }
            }
            _ => {}
        }
    }
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

/// Smashline 1 `#[fighter_frame(agent = FIGHTER_KIND_...)]` or entry `#[status_script(...)]`.
fn attribute_fighter(item: &syn::ItemFn) -> Option<String> {
    item.attrs.iter().find_map(|attr| {
        let path = attr.path().segments.last()?.ident.to_string();
        let tokens = attr.meta.require_list().ok()?.tokens.to_string();
        if path == "fighter_frame" {
            let kind = tokens.split("FIGHTER_KIND_").nth(1)?;
            let kind: String = kind
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            return (!kind.is_empty()).then(|| kind.to_ascii_lowercase());
        }
        if path == "status_script" && tokens.contains("FIGHTER_STATUS_KIND_ENTRY") {
            if let Some(after) = tokens.split("agent").nth(1) {
                if let Some(start) = after.find('"') {
                    let rest = &after[start + 1..];
                    if let Some(end) = rest.find('"') {
                        let agent = &rest[..end];
                        if !agent.is_empty() {
                            return Some(agent.to_ascii_lowercase());
                        }
                    }
                }
            }
        }
        None
    })
}

fn path_name(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Path(path) => Some(path.path.segments.last()?.ident.to_string()),
        syn::Expr::Reference(reference) => path_name(&reference.expr),
        syn::Expr::Paren(paren) => path_name(&paren.expr),
        _ => None,
    }
}

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

fn chain_agent(expr: &syn::Expr, bindings: &HashMap<String, String>) -> Option<String> {
    match expr {
        syn::Expr::MethodCall(call) => chain_agent(&call.receiver, bindings),
        syn::Expr::Path(path) if path.path.segments.len() == 1 => {
            bindings.get(&path.path.segments[0].ident.to_string()).cloned()
        }
        syn::Expr::Reference(reference) => chain_agent(&reference.expr, bindings),
        syn::Expr::Paren(paren) => chain_agent(&paren.expr, bindings),
        _ => agent_new(expr),
    }
}

/// Find `.on_line(..., cb)`, `.on_start(cb)`, and `.status(..., *FIGHTER_STATUS_KIND_ENTRY, cb)`.
fn find_callbacks(
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
                    find_callbacks_expr(&init.expr, bindings, found);
                }
            }
            syn::Stmt::Expr(expr, _) => find_callbacks_expr(expr, bindings, found),
            _ => {}
        }
    }
}

fn find_callbacks_expr(
    expr: &syn::Expr,
    bindings: &mut HashMap<String, String>,
    found: &mut impl FnMut(Option<String>, String),
) {
    match expr {
        syn::Expr::MethodCall(call) => {
            let method = call.method.to_string();
            if method == "on_line" && call.args.len() >= 2 {
                if let Some(callback) = path_name(&call.args[1]) {
                    found(chain_agent(&call.receiver, bindings), callback);
                }
            } else if method == "on_start" && !call.args.is_empty() {
                if let Some(callback) = path_name(&call.args[0]) {
                    found(chain_agent(&call.receiver, bindings), callback);
                }
            } else if method == "status" && call.args.len() >= 3 {
                let status_words = words(&call.args[1]);
                if status_words
                    .iter()
                    .any(|w| w == "FIGHTER_STATUS_KIND_ENTRY")
                {
                    if let Some(callback) = path_name(&call.args[2]) {
                        found(chain_agent(&call.receiver, bindings), callback);
                    }
                }
            }
            find_callbacks_expr(&call.receiver, bindings, found);
        }
        syn::Expr::Block(block) => find_callbacks(&block.block, bindings, found),
        syn::Expr::Unsafe(block) => find_callbacks(&block.block, bindings, found),
        syn::Expr::If(expr_if) => {
            find_callbacks(&expr_if.then_branch, bindings, found);
            if let Some((_, other)) = &expr_if.else_branch {
                find_callbacks_expr(other, bindings, found);
            }
        }
        _ => {}
    }
}

#[derive(Clone)]
enum LocalVal {
    Num(f32),
    Vec3([f32; 3]),
    Str(String),
    HurtboxList(Vec<ParsedHurtbox>),
    Unknown,
}

type Env = Vec<(String, LocalVal)>;

struct Walker<'a> {
    functions: &'a HashMap<String, Vec<(&'a Path, &'a syn::ItemFn)>>,
    const_exprs: &'a HashMap<String, &'a syn::Expr>,
    consts: &'a crate::project_consts::ProjectConsts,
    found: Vec<SourceHurtbox>,
    stack: HashSet<String>,
}

impl<'a> Walker<'a> {
    fn walk_fn(&mut self, file: &'a Path, item: &'a syn::ItemFn, args: Vec<LocalVal>, depth: usize) {
        let name = item.sig.ident.to_string();
        if depth > MAX_DEPTH || !self.stack.insert(name.clone()) {
            return;
        }
        let mut env: Env = Vec::new();
        for (index, input) in item.sig.inputs.iter().enumerate() {
            if let syn::FnArg::Typed(typed) = input {
                if let syn::Pat::Ident(param) = &*typed.pat {
                    let val = args.get(index).cloned().unwrap_or(LocalVal::Unknown);
                    env.push((param.ident.to_string(), val));
                }
            }
        }
        self.walk_block(file, &item.block, &mut env, depth);
        self.stack.remove(&name);
    }

    fn walk_block(&mut self, file: &'a Path, block: &'a syn::Block, env: &mut Env, depth: usize) {
        let scope = env.len();
        for stmt in &block.stmts {
            match stmt {
                syn::Stmt::Local(local) => {
                    let val = local
                        .init
                        .as_ref()
                        .map(|init| self.eval_local(&init.expr, env))
                        .unwrap_or(LocalVal::Unknown);
                    if let Some(init) = &local.init {
                        self.walk_expr(file, &init.expr, env, depth);
                    }
                    if let syn::Pat::Ident(name) = &local.pat {
                        env.push((name.ident.to_string(), val));
                    } else if let syn::Pat::Type(typed) = &local.pat {
                        if let syn::Pat::Ident(name) = &*typed.pat {
                            env.push((name.ident.to_string(), val));
                        }
                    }
                }
                syn::Stmt::Expr(expr, _) => self.walk_expr(file, expr, env, depth),
                _ => {}
            }
        }
        env.truncate(scope);
    }

    fn walk_expr(&mut self, file: &'a Path, expr: &'a syn::Expr, env: &mut Env, depth: usize) {
        match expr {
            syn::Expr::Block(block) => self.walk_block(file, &block.block, env, depth),
            syn::Expr::Unsafe(block) => self.walk_block(file, &block.block, env, depth),
            syn::Expr::Paren(paren) => self.walk_expr(file, &paren.expr, env, depth),
            syn::Expr::If(expr_if) => {
                // Skip branches that only run on victory/results/demo screens, while walking
                // both ordinary and ENTRY/reset branches.
                match self.results_only_condition(&expr_if.cond) {
                    Some(true) => {
                        if let Some((_, other)) = &expr_if.else_branch {
                            self.walk_expr(file, other, env, depth);
                        }
                    }
                    Some(false) => {
                        self.walk_block(file, &expr_if.then_branch, env, depth);
                    }
                    None => {
                        self.walk_block(file, &expr_if.then_branch, env, depth);
                        if let Some((_, other)) = &expr_if.else_branch {
                            self.walk_expr(file, other, env, depth);
                        }
                    }
                }
            }
            syn::Expr::Call(call) => {
                for arg in &call.args {
                    self.walk_expr(file, arg, env, depth);
                }
                let syn::Expr::Path(func) = &*call.func else {
                    return;
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

                // 1. Direct `FighterUtil::set_hit_data(boma, idx, group, &p1, &p2, r, bone, ...)`.
                if matches!(tail.as_slice(), ["FighterUtil", "set_hit_data"] | ["set_hit_data"]) {
                    if let Some(hurtbox) = self.parse_set_hit_data(call, file, env) {
                        self.record(hurtbox);
                    }
                    return;
                }

                let Some(name) = segments.last() else {
                    return;
                };

                // 2. Helper slice calls like `set_hurtboxes(boma, &[Hurtbox::capsule(...), ...])`.
                let callee = pick_fn(self.functions, name, file);
                let is_hurtbox_helper = name.to_ascii_lowercase().contains("hurtbox")
                    || callee.is_some_and(|(_, item)| fn_mentions_set_hit_data(item));
                if is_hurtbox_helper {
                    for arg in &call.args {
                        if let Some(list) = self.eval_hurtbox_list(arg, env) {
                            for (idx, h) in list.into_iter().enumerate() {
                                self.record(SourceHurtbox {
                                    index: idx,
                                    bone_name: h.bone_name,
                                    endpoint1: h.endpoint1,
                                    endpoint2: h.endpoint2,
                                    radius: h.radius,
                                    file: file.to_path_buf(),
                                    line: h.line,
                                });
                            }
                            return;
                        }
                    }
                }

                // 3. Follow ordinary helper functions.
                if let Some((callee_file, callee_item)) = callee {
                    let args = call
                        .args
                        .iter()
                        .map(|arg| self.eval_local(arg, env))
                        .collect();
                    self.walk_fn(callee_file, callee_item, args, depth + 1);
                }
            }
            syn::Expr::MethodCall(call) => {
                self.walk_expr(file, &call.receiver, env, depth);
                for arg in &call.args {
                    self.walk_expr(file, arg, env, depth);
                }
            }
            syn::Expr::Match(expr_match) => {
                for arm in &expr_match.arms {
                    self.walk_expr(file, &arm.body, env, depth);
                }
            }
            syn::Expr::Loop(expr_loop) => self.walk_block(file, &expr_loop.body, env, depth),
            syn::Expr::While(expr_while) => self.walk_block(file, &expr_while.body, env, depth),
            syn::Expr::ForLoop(expr_for) => self.walk_block(file, &expr_for.body, env, depth),
            _ => {}
        }
    }

    fn record(&mut self, hurtbox: SourceHurtbox) {
        if let Some(existing) = self.found.iter_mut().find(|h| h.index == hurtbox.index) {
            *existing = hurtbox;
        } else {
            self.found.push(hurtbox);
        }
    }

    fn parse_set_hit_data(
        &self,
        call: &syn::ExprCall,
        file: &Path,
        env: &Env,
    ) -> Option<SourceHurtbox> {
        let index_f = self.eval_num(call.args.iter().nth(1)?, env)?;
        if !index_f.is_finite() || index_f < 0.0 {
            return None;
        }
        let index = index_f.round() as usize;
        let endpoint1 = self.eval_vec3(call.args.iter().nth(3)?, env)?;
        let endpoint2 = self.eval_vec3(call.args.iter().nth(4)?, env)?;
        let radius = self.eval_num(call.args.iter().nth(5)?, env)?;
        if !radius.is_finite() || radius < 0.0 {
            return None;
        }
        let bone_name = self.eval_bone_name(call.args.iter().nth(6)?, env)?;
        Some(SourceHurtbox {
            index,
            bone_name,
            endpoint1,
            endpoint2,
            radius,
            file: file.to_path_buf(),
            line: call.span().start().line,
        })
    }

    fn eval_local(&self, expr: &syn::Expr, env: &Env) -> LocalVal {
        if let Some(v) = self.eval_num(expr, env) {
            return LocalVal::Num(v);
        }
        if let Some(v) = self.eval_vec3(expr, env) {
            return LocalVal::Vec3(v);
        }
        if let Some(s) = self.eval_bone_name(expr, env) {
            return LocalVal::Str(s);
        }
        if let Some(list) = self.eval_hurtbox_list(expr, env) {
            return LocalVal::HurtboxList(list);
        }
        LocalVal::Unknown
    }

    fn eval_hurtbox_list(&self, expr: &syn::Expr, env: &Env) -> Option<Vec<ParsedHurtbox>> {
        match expr {
            syn::Expr::Reference(reference) => self.eval_hurtbox_list(&reference.expr, env),
            syn::Expr::Paren(paren) => self.eval_hurtbox_list(&paren.expr, env),
            syn::Expr::Group(group) => self.eval_hurtbox_list(&group.expr, env),
            syn::Expr::Array(array) => {
                let mut items = Vec::new();
                for elem in &array.elems {
                    items.push(self.eval_hurtbox_item(elem, env)?);
                }
                (!items.is_empty()).then_some(items)
            }
            syn::Expr::Path(path) => {
                let name = path.path.segments.last()?.ident.to_string();
                if path.path.segments.len() == 1 {
                    if let Some((_, LocalVal::HurtboxList(list))) =
                        env.iter().rev().find(|(n, _)| *n == name)
                    {
                        return Some(list.clone());
                    }
                }
                let const_expr = self.const_exprs.get(&name)?;
                self.eval_hurtbox_list(const_expr, env)
            }
            _ => None,
        }
    }

    fn eval_hurtbox_item(&self, expr: &syn::Expr, env: &Env) -> Option<ParsedHurtbox> {
        match expr {
            syn::Expr::Paren(paren) => self.eval_hurtbox_item(&paren.expr, env),
            syn::Expr::Group(group) => self.eval_hurtbox_item(&group.expr, env),
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
                let method = segments.last()?.as_str();
                match method {
                    "capsule" | "new" if call.args.len() >= 4 => {
                        let bone_name = self.eval_bone_name(&call.args[0], env)?;
                        let endpoint1 = self.eval_vec3(&call.args[1], env)?;
                        let endpoint2 = self.eval_vec3(&call.args[2], env)?;
                        let radius = self.eval_num(&call.args[3], env)?;
                        (radius.is_finite() && radius >= 0.0).then_some(ParsedHurtbox {
                            bone_name,
                            endpoint1,
                            endpoint2,
                            radius,
                            line: call.span().start().line,
                        })
                    }
                    "sphere" if call.args.len() >= 3 => {
                        let bone_name = self.eval_bone_name(&call.args[0], env)?;
                        let center = self.eval_vec3(&call.args[1], env)?;
                        let radius = self.eval_num(&call.args[2], env)?;
                        (radius.is_finite() && radius >= 0.0).then_some(ParsedHurtbox {
                            bone_name,
                            endpoint1: center,
                            endpoint2: center,
                            radius,
                            line: call.span().start().line,
                        })
                    }
                    _ => None,
                }
            }
            syn::Expr::Struct(expr_struct) => {
                let mut bone_name = None;
                let mut endpoint1 = None;
                let mut endpoint2 = None;
                let mut radius = None;
                for field in &expr_struct.fields {
                    let syn::Member::Named(ident) = &field.member else {
                        continue;
                    };
                    match ident.to_string().as_str() {
                        "bone" | "node" | "bone_name" => {
                            bone_name = self.eval_bone_name(&field.expr, env);
                        }
                        "start" | "offset1" | "endpoint1" => {
                            endpoint1 = self.eval_vec3(&field.expr, env);
                        }
                        "end" | "offset2" | "endpoint2" => {
                            endpoint2 = self.eval_vec3(&field.expr, env);
                        }
                        "radius" | "size" => {
                            radius = self.eval_num(&field.expr, env);
                        }
                        _ => {}
                    }
                }
                let radius = radius?;
                (radius.is_finite() && radius >= 0.0).then_some(ParsedHurtbox {
                    bone_name: bone_name?,
                    endpoint1: endpoint1?,
                    endpoint2: endpoint2.or(endpoint1)?,
                    radius,
                    line: expr_struct.span().start().line,
                })
            }
            _ => None,
        }
    }

    fn eval_num(&self, expr: &syn::Expr, env: &Env) -> Option<f32> {
        match expr {
            syn::Expr::Lit(lit) => match &lit.lit {
                syn::Lit::Float(f) => f.base10_parse::<f32>().ok(),
                syn::Lit::Int(i) => i.base10_parse::<f32>().ok(),
                _ => None,
            },
            syn::Expr::Paren(paren) => self.eval_num(&paren.expr, env),
            syn::Expr::Group(group) => self.eval_num(&group.expr, env),
            syn::Expr::Cast(cast) => self.eval_num(&cast.expr, env),
            syn::Expr::Unary(syn::ExprUnary {
                op: syn::UnOp::Neg(_),
                expr,
                ..
            }) => self.eval_num(expr, env).map(|v| -v),
            syn::Expr::Binary(binary) => {
                let (left, right) = (
                    self.eval_num(&binary.left, env)?,
                    self.eval_num(&binary.right, env)?,
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
                    if let Some((_, val)) = env.iter().rev().find(|(n, _)| *n == name) {
                        return match val {
                            LocalVal::Num(n) => Some(*n),
                            _ => None,
                        };
                    }
                }
                self.consts.get(&name)?.number().map(|v| v as f32)
            }
            _ => None,
        }
    }

    fn eval_vec3(&self, expr: &syn::Expr, env: &Env) -> Option<[f32; 3]> {
        match expr {
            syn::Expr::Reference(reference) => self.eval_vec3(&reference.expr, env),
            syn::Expr::Paren(paren) => self.eval_vec3(&paren.expr, env),
            syn::Expr::Group(group) => self.eval_vec3(&group.expr, env),
            syn::Expr::Array(array) if array.elems.len() == 3 => Some([
                self.eval_num(&array.elems[0], env)?,
                self.eval_num(&array.elems[1], env)?,
                self.eval_num(&array.elems[2], env)?,
            ]),
            syn::Expr::Struct(expr_struct) => {
                let mut x = None;
                let mut y = None;
                let mut z = None;
                for field in &expr_struct.fields {
                    let syn::Member::Named(ident) = &field.member else {
                        continue;
                    };
                    match ident.to_string().as_str() {
                        "x" => x = self.eval_num(&field.expr, env),
                        "y" => y = self.eval_num(&field.expr, env),
                        "z" => z = self.eval_num(&field.expr, env),
                        _ => {}
                    }
                }
                Some([x?, y?, z?])
            }
            syn::Expr::Call(call) => {
                let syn::Expr::Path(func) = &*call.func else {
                    return None;
                };
                let last = func.path.segments.last()?.ident.to_string();
                if last == "zero" && call.args.is_empty() {
                    return Some([0.0, 0.0, 0.0]);
                }
                if last == "new" && call.args.len() == 3 {
                    return Some([
                        self.eval_num(&call.args[0], env)?,
                        self.eval_num(&call.args[1], env)?,
                        self.eval_num(&call.args[2], env)?,
                    ]);
                }
                None
            }
            syn::Expr::Path(path) if path.path.segments.len() == 1 => {
                let name = path.path.segments[0].ident.to_string();
                env.iter().rev().find_map(|(n, val)| {
                    (*n == name).then_some(match val {
                        LocalVal::Vec3(v) => Some(*v),
                        _ => None,
                    })?
                })
            }
            _ => None,
        }
    }

    fn eval_bone_name(&self, expr: &syn::Expr, env: &Env) -> Option<String> {
        match expr {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) => Some(s.value()),
            syn::Expr::Reference(reference) => self.eval_bone_name(&reference.expr, env),
            syn::Expr::Paren(paren) => self.eval_bone_name(&paren.expr, env),
            syn::Expr::Group(group) => self.eval_bone_name(&group.expr, env),
            syn::Expr::Call(call) => {
                // `Hash40::new("waist")` or `hash40("waist")`.
                let syn::Expr::Path(func) = &*call.func else {
                    return None;
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
                if matches!(tail.as_slice(), ["Hash40", "new"] | ["hash40"]) {
                    return self.eval_bone_name(call.args.first()?, env);
                }
                None
            }
            syn::Expr::Path(path) if path.path.segments.len() == 1 => {
                let name = path.path.segments[0].ident.to_string();
                env.iter().rev().find_map(|(n, val)| {
                    (*n == name).then_some(match val {
                        LocalVal::Str(s) => Some(s.clone()),
                        _ => None,
                    })?
                })
            }
            _ => None,
        }
    }

    /// Returns `Some(true)` if `cond` only holds on results/demo screens (skip `then`, walk `else`),
    /// `Some(false)` if it is the negation of a results-only check (walk `then`, skip `else`),
    /// or `None` otherwise (walk both branches).
    fn results_only_condition(&self, cond: &syn::Expr) -> Option<bool> {
        match cond {
            syn::Expr::Paren(paren) => self.results_only_condition(&paren.expr),
            syn::Expr::Unary(syn::ExprUnary {
                op: syn::UnOp::Not(_),
                expr,
                ..
            }) => self.results_only_condition(expr).map(|v| !v),
            syn::Expr::Binary(binary) => match binary.op {
                syn::BinOp::Eq(_) | syn::BinOp::Ne(_) => {
                    let equal = matches!(binary.op, syn::BinOp::Eq(_));
                    let (left, right) = (words(&binary.left), words(&binary.right));
                    if only_results_statuses(&left) || only_results_statuses(&right) {
                        return Some(equal);
                    }
                    None
                }
                _ => None,
            },
            syn::Expr::MethodCall(call)
                if call.method == "is_status_one_of" || call.method == "is_status" =>
            {
                only_results_statuses(&words(cond)).then_some(true)
            }
            _ => None,
        }
    }
}

fn fn_mentions_set_hit_data(item: &syn::ItemFn) -> bool {
    fn walk_block(block: &syn::Block) -> bool {
        block.stmts.iter().any(|stmt| match stmt {
            syn::Stmt::Local(local) => local
                .init
                .as_ref()
                .is_some_and(|init| walk_expr(&init.expr)),
            syn::Stmt::Expr(expr, _) => walk_expr(expr),
            _ => false,
        })
    }
    fn walk_expr(expr: &syn::Expr) -> bool {
        match expr {
            syn::Expr::Call(call) => {
                if let syn::Expr::Path(func) = &*call.func {
                    if func
                        .path
                        .segments
                        .last()
                        .is_some_and(|s| s.ident == "set_hit_data")
                    {
                        return true;
                    }
                }
                call.args.iter().any(walk_expr)
            }
            syn::Expr::Block(b) => walk_block(&b.block),
            syn::Expr::Unsafe(b) => walk_block(&b.block),
            syn::Expr::ForLoop(f) => walk_block(&f.body),
            syn::Expr::While(w) => walk_block(&w.body),
            syn::Expr::Loop(l) => walk_block(&l.body),
            syn::Expr::If(i) => {
                walk_block(&i.then_branch)
                    || i.else_branch.as_ref().is_some_and(|(_, e)| walk_expr(e))
            }
            syn::Expr::Match(m) => m.arms.iter().any(|arm| walk_expr(&arm.body)),
            syn::Expr::Paren(p) => walk_expr(&p.expr),
            _ => false,
        }
    }
    walk_block(&item.block)
}

fn words(expr: &syn::Expr) -> Vec<String> {
    fn walk(expr: &syn::Expr, out: &mut Vec<String>) {
        match expr {
            syn::Expr::Path(path) => {
                out.extend(path.path.segments.iter().map(|s| s.ident.to_string()));
            }
            syn::Expr::Call(call) => {
                walk(&call.func, out);
                call.args.iter().for_each(|arg| walk(arg, out));
            }
            syn::Expr::MethodCall(call) => {
                walk(&call.receiver, out);
                out.push(call.method.to_string());
                call.args.iter().for_each(|arg| walk(arg, out));
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

fn only_results_statuses(words: &[String]) -> bool {
    let statuses: Vec<&String> = words
        .iter()
        .filter(|w| w.starts_with("FIGHTER_STATUS_KIND_"))
        .collect();
    !statuses.is_empty()
        && statuses
            .iter()
            .all(|status| RESULTS_ONLY_STATUSES.contains(&status.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_texts(
        files: &[(&str, &str)],
        fighter: Option<&str>,
    ) -> HashMap<String, Vec<SourceHurtbox>> {
        let sources: Vec<(PathBuf, String)> = files
            .iter()
            .map(|(path, text)| (PathBuf::from(path), text.to_string()))
            .collect();
        let consts = crate::project_consts::ProjectConsts::scan(&sources);
        scan(&sources, |_| fighter.map(str::to_string), &consts)
    }

    #[test]
    fn shyguy_entry_set_hurtboxes_extracts_elongated_waist_and_zero_legs() {
        let util = r#"
pub unsafe fn set_hurtboxes(module_accessor: *mut BattleObjectModuleAccessor, hurtboxes: &[Hurtbox]) {
    for (i, h) in hurtboxes.iter().enumerate() {
        FighterUtil::set_hit_data(
            module_accessor, i as i32, 0,
            &h.start, &h.end,
            h.radius,
            h.bone,
            h.part,
            h.height,
            HitStatus(*HIT_STATUS_NORMAL),
            smash::app::CollisionShapeType(*COLLISION_SHAPE_TYPE_CAPSULE),
        );
    }
}
"#;
        let frame = r#"
unsafe extern "C" fn shyguy_frame(fighter: &mut L2CFighterCommon) {
    if status_kind == *FIGHTER_STATUS_KIND_ENTRY || is_reset() {
        set_hurtboxes(fighter.module_accessor, &[
            Hurtbox::capsule("shoulderl", [0.0,  0.0, 0.0], [1.0, 0.0, 0.0], 1.0, *COLLISION_PART_BODY,      *HIT_HEIGHT_CENTER),
            Hurtbox::capsule("shoulderr", [0.0,  0.0, 0.0], [1.0, 0.0, 0.0], 1.0, *COLLISION_PART_BODY,      *HIT_HEIGHT_CENTER),
            Hurtbox::capsule("handl",     [0.0,  0.0, 0.0], [-0.8, 0.0, 0.0], 1.0, *COLLISION_PART_ETC,      *HIT_HEIGHT_CENTER),
            Hurtbox::capsule("handr",     [0.0,  0.0, 0.0], [-0.8, 0.0, 0.0], 1.0, *COLLISION_PART_ETC,      *HIT_HEIGHT_CENTER),
            Hurtbox::capsule("waist",     [0.0,  0.0, 0.0], [4.2, 0.0, 0.0], 4.7, *COLLISION_PART_BODY,      *HIT_HEIGHT_CENTER),
            Hurtbox::capsule("legl",      [0.0, -0.7, 0.0], [0.0, 1.8, 0.0], 0.0, *COLLISION_PART_BODY_LEGS, *HIT_HEIGHT_LOW),
            Hurtbox::capsule("legr",      [0.0, -0.7, 0.0], [0.0, 1.8, 0.0], 0.0, *COLLISION_PART_BODY_LEGS, *HIT_HEIGHT_LOW),
        ]);
    }
}
pub(super) fn install(agent: &mut Agent) {
    agent.on_line(Main, shyguy_frame);
}
"#;
        let found = scan_texts(&[("util.rs", util), ("frame.rs", frame)], Some("pacman"));
        let list = &found["pacman"];
        assert_eq!(list.len(), 7);
        assert_eq!(list[4].index, 4);
        assert_eq!(list[4].bone_name, "waist");
        assert_eq!(list[4].endpoint1, [0.0, 0.0, 0.0]);
        assert_eq!(list[4].endpoint2, [4.2, 0.0, 0.0]);
        assert_eq!(list[4].radius, 4.7);
        assert_eq!(list[5].bone_name, "legl");
        assert_eq!(list[5].radius, 0.0);
    }

    #[test]
    fn direct_fighter_util_set_hit_data_with_consts_is_extracted() {
        let frame = r#"
pub const WAIST_TOP: f32 = 4.2;
pub const WAIST_RAD: f32 = 4.7;
#[fighter_frame(agent = FIGHTER_KIND_PACMAN)]
fn frame(fighter: &mut L2CFighterCommon) {
    if StatusModule::status_kind(fighter.module_accessor) == *FIGHTER_STATUS_KIND_WIN {
        FighterUtil::set_hit_data(
            fighter.module_accessor, 4, 0,
            &Vector3f { x: 0.0, y: 0.0, z: 0.0 },
            &Vector3f { x: 0.0, y: 0.0, z: 0.0 },
            1.0, Hash40::new("waist"),
        );
        return;
    }
    FighterUtil::set_hit_data(
        fighter.module_accessor,
        4,
        0,
        &Vector3f { x: 0.0, y: 0.0, z: 0.0 },
        &Vector3f { x: WAIST_TOP, y: 0.0, z: 0.0 },
        WAIST_RAD,
        Hash40::new("waist"),
        CollisionPart(*COLLISION_PART_BODY),
        HitHeight(*HIT_HEIGHT_CENTER),
        HitStatus(*HIT_STATUS_NORMAL),
        CollisionShapeType(*COLLISION_SHAPE_TYPE_CAPSULE),
    );
}
"#;
        let found = scan_texts(&[("frame.rs", frame)], None);
        let list = &found["pacman"];
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].index, 4);
        assert_eq!(list[0].bone_name, "waist");
        assert_eq!(list[0].endpoint2, [4.2, 0.0, 0.0]);
        assert_eq!(list[0].radius, 4.7);
    }
}
