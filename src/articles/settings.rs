//! An article's size, speed, life, gravity and power, read from its status code.
//!
//! Movesets set these in an article's smashline status scripts rather than in params:
//!
//! ```text
//! ModelModule::set_scale(acc, v)                    PostureModule::set_scale(acc, v, _)
//! sv_kinetic_energy!(op, agent, RESERVE, x, y)      the lua-stack form of the same call
//! WorkModule::set_int(acc, v, *WEAPON_INSTANCE_WORK_ID_INT_LIFE)    weapon.set_life(v)
//! AttackModule::set_power_mul(acc, v)
//! ```
//!
//! Each value is classified by where it comes from (a literal, a const, a param, or something
//! computed) and carries the byte span of its expression in the file, so a writer can splice an
//! edit in. A computed value is shown with its text and never overwritten. A call the reader does
//! not model is kept under [`SettingField::OtherMovement`] with its text, never dropped.
//!
//! This reader is pure and has no UI.

use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::acmd_src::StatusSite;
use crate::project_consts::ProjectConsts;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingField {
    Scale,
    SpeedX,
    SpeedY,
    AccelX,
    AccelY,
    /// The speed the article settles at, not its top speed.
    StableSpeedX,
    StableSpeedY,
    MaxSpeedX,
    MaxSpeedY,
    BrakeX,
    BrakeY,
    SpeedMul,
    Life,
    PowerMul,
    /// A negated y acceleration, stored as its magnitude.
    Gravity,
    /// A `sv_kinetic_energy` op this reader does not model, kept as text.
    OtherMovement,
}

impl SettingField {
    pub fn label(self) -> &'static str {
        match self {
            SettingField::Scale => "Size",
            SettingField::SpeedX => "Speed X",
            SettingField::SpeedY => "Speed Y",
            SettingField::AccelX => "Acceleration X",
            SettingField::AccelY => "Acceleration Y",
            SettingField::StableSpeedX => "Settling speed X",
            SettingField::StableSpeedY => "Settling speed Y",
            SettingField::MaxSpeedX => "Max speed X",
            SettingField::MaxSpeedY => "Max speed Y",
            SettingField::BrakeX => "Brake X",
            SettingField::BrakeY => "Brake Y",
            SettingField::SpeedMul => "Speed multiplier",
            SettingField::Life => "Life",
            SettingField::PowerMul => "Power multiplier",
            SettingField::Gravity => "Gravity",
            SettingField::OtherMovement => "Other movement",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SettingValue {
    Literal(f64),
    /// A const by name; `value` is its number when the project defines it.
    Const { name: String, value: Option<f64> },
    /// `WorkModule::get_param_*(acc, hash40(block), hash40(field))`, directly or through a `let`.
    Param { block: String, field: String },
    /// Anything else, as written. When it reads params through `let`s, the text ends with
    /// `(reads block.field, ...)` so the origin is always shown.
    Computed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    pub field: SettingField,
    pub value: SettingValue,
    /// The status kind as written (`StatusSite::kind_expr`).
    pub status: String,
    pub phase: String,
    /// The file the value is written in: the status's, or a followed helper's.
    pub file: PathBuf,
    /// The value expression's bytes in `file`. For [`SettingField::OtherMovement`], the whole call.
    pub span: Range<usize>,
    /// A trailing `* lr`, `* facing` or `* PostureModule::lr(..)` was stripped from the value.
    pub facing_factor: bool,
    /// `(block, field)` of every `get_param_*` the value reads, directly or through `let`s.
    pub referenced_params: Vec<(String, String)>,
}

/// The settings `statuses` write, each status's body read along with the project functions it
/// calls (one level deep, through `helpers`, which maps the calling function's file and a function
/// name to that function's file, its byte span in that file, and its source text). The caller's
/// file is passed because per-module names (`main_loop`, `init`) repeat across a project.
pub fn read_settings(
    statuses: &[(&StatusSite, String)],
    consts: &ProjectConsts,
    helpers: &dyn Fn(&Path, &str) -> Option<(PathBuf, Range<usize>, String)>,
) -> Vec<Setting> {
    let mut out = Vec::new();
    let mut file_consts_cache: std::collections::HashMap<PathBuf, Option<ProjectConsts>> =
        std::collections::HashMap::new();
    let mut local_consts_for = |path: &Path| -> Option<ProjectConsts> {
        file_consts_cache
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                let text = std::fs::read_to_string(path).ok()?;
                Some(ProjectConsts::scan(&[(path.to_path_buf(), text)]))
            })
            .clone()
    };
    for (site, body) in statuses {
        let status = Status { kind: &site.kind_expr, phase: &site.phase };
        let file_consts = local_consts_for(&site.file);
        let called = scan(
            body,
            &site.function,
            &site.file,
            site.span.start,
            consts,
            file_consts.as_ref(),
            status,
            &mut out,
        );
        let mut followed: Vec<&str> = Vec::new();
        for name in &called {
            if name == &site.function || followed.contains(&name.as_str()) {
                continue;
            }
            followed.push(name);
            if let Some((file, span, text)) = helpers(&site.file, name) {
                let helper_consts = local_consts_for(&file);
                // One level only: what the helper calls is not followed.
                scan(
                    &text,
                    name,
                    &file,
                    span.start,
                    consts,
                    helper_consts.as_ref(),
                    status,
                    &mut out,
                );
            }
        }
    }
    out
}

#[derive(Clone, Copy)]
struct Status<'a> {
    kind: &'a str,
    phase: &'a str,
}

/// Records the settings `text` (the function `name`, at byte `base` of `file`) writes, and returns
/// the names it references that may be project functions.
fn scan(
    text: &str,
    name: &str,
    file: &Path,
    base: usize,
    consts: &ProjectConsts,
    file_consts: Option<&ProjectConsts>,
    status: Status,
    out: &mut Vec<Setting>,
) -> Vec<String> {
    let Some((start, functions)) = parse_functions(text, name) else {
        out.push(Setting {
            field: SettingField::OtherMovement,
            value: SettingValue::Computed("Visionary could not read this status function".into()),
            status: status.kind.to_string(),
            phase: status.phase.to_string(),
            file: file.to_path_buf(),
            span: base..base + text.len(),
            facing_factor: false,
            referenced_params: Vec::new(),
        });
        return Vec::new();
    };
    let (text, base) = (&text[start..], base + start);
    let lines = LineIndex::new(text);
    let mut called = Vec::new();
    for function in &functions {
        let mut lets = LetCollector { lines: &lines, lets: Vec::new() };
        syn::visit::Visit::visit_item_fn(&mut lets, function);
        let mut scanner = Scanner {
            ctx: Ctx { text, lines: &lines, lets: &lets.lets, consts, file_consts },
            file,
            base,
            status,
            lua_stack: Vec::new(),
            out: &mut *out,
            called: &mut called,
        };
        syn::visit::Visit::visit_block(&mut scanner, &function.block);
    }
    called
}

/// The functions in `text`, with the byte offset the parsed text starts at. A span can start
/// before the item (`} unsafe extern "C" fn x(..)`), so when the whole text is not a file holding
/// a function, parsing is retried from each keyword a function item can start with. A retry is
/// accepted only for the function `name`: a function nested in a broken one is not it.
fn parse_functions(text: &str, name: &str) -> Option<(usize, Vec<syn::ItemFn>)> {
    if let Ok(file) = syn::parse_file(text) {
        let functions: Vec<syn::ItemFn> = file
            .items
            .into_iter()
            .filter_map(|item| match item {
                syn::Item::Fn(function) => Some(function),
                _ => None,
            })
            .collect();
        if !functions.is_empty() {
            return Some((0, functions));
        }
    }
    let one_function = |input: syn::parse::ParseStream| -> syn::Result<syn::ItemFn> {
        let function: syn::ItemFn = input.parse()?;
        input.parse::<proc_macro2::TokenStream>()?;
        Ok(function)
    };
    let starts = ["#[", "pub", "unsafe", "extern", "async", "const", "fn"];
    let mut candidates: Vec<usize> = starts
        .iter()
        .flat_map(|keyword| text.match_indices(keyword).map(|(at, _)| at))
        .filter(|&at| at == 0 || !text[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_'))
        .collect();
    candidates.sort_unstable();
    candidates.dedup();
    candidates.into_iter().take(16).find_map(|at| {
        use syn::parse::Parser;
        let function = one_function.parse_str(&text[at..]).ok()?;
        (function.sig.ident == name).then(|| (at, vec![function]))
    })
}

/// Line starts of a text, to turn a span's line and (character) column into a byte offset.
struct LineIndex<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        LineIndex { text, starts }
    }

    fn offset(&self, at: proc_macro2::LineColumn) -> usize {
        let Some(&start) = self.starts.get(at.line.saturating_sub(1)) else {
            return self.text.len();
        };
        self.text[start..]
            .char_indices()
            .nth(at.column)
            .map_or(self.text.len(), |(i, _)| start + i)
    }

    fn range(&self, span: proc_macro2::Span) -> Range<usize> {
        self.offset(span.start())..self.offset(span.end())
    }
}

/// `let NAME = INIT;` bindings of a function, with the byte offset each ends at, so a use reads the
/// binding in scope: one that ends before it (a shadowing `let x = x * 2.0;` reads the earlier `x`).
struct LetCollector<'a> {
    lines: &'a LineIndex<'a>,
    lets: Vec<(String, usize, syn::Expr)>,
}

impl<'ast> syn::visit::Visit<'ast> for LetCollector<'_> {
    fn visit_local(&mut self, local: &'ast syn::Local) {
        let mut pat = &local.pat;
        if let syn::Pat::Type(typed) = pat {
            pat = &typed.pat;
        }
        if let (syn::Pat::Ident(ident), Some(init)) = (pat, &local.init) {
            let at = self.lines.offset(syn::spanned::Spanned::span(local).end());
            self.lets.push((ident.ident.to_string(), at, (*init.expr).clone()));
        }
        syn::visit::visit_local(self, local);
    }
}

/// What classifying a value needs: the function's text, its `let`s and the project's consts.
struct Ctx<'a> {
    text: &'a str,
    lines: &'a LineIndex<'a>,
    lets: &'a [(String, usize, syn::Expr)],
    consts: &'a ProjectConsts,
    /// Consts defined in the scanned status/helper's own file, so private consts whose names
    /// repeat in other modules (e.g. `LAUNCH_VY`, `LIFETIME_FRAMES`) still resolve unambiguously.
    file_consts: Option<&'a ProjectConsts>,
}

struct Classified {
    value: SettingValue,
    /// In the scanned text.
    span: Range<usize>,
    facing_factor: bool,
    referenced_params: Vec<(String, String)>,
}

impl Ctx<'_> {
    fn range(&self, node: &impl syn::spanned::Spanned) -> Range<usize> {
        self.lines.range(node.span())
    }

    fn source(&self, node: &impl syn::spanned::Spanned) -> &str {
        self.text.get(self.range(node)).unwrap_or_default()
    }

    /// The `let` binding `name` in scope at byte `at`, with the offset it ends at.
    fn binding(&self, name: &str, at: usize) -> Option<(usize, &syn::Expr)> {
        self.lets
            .iter()
            .filter(|(n, end, _)| n == name && *end <= at)
            .max_by_key(|(_, end, _)| *end)
            .map(|(_, end, init)| (*end, init))
    }

    /// A local name used as `expr`, with the expression it is bound to.
    fn local(&self, expr: &syn::Expr) -> Option<&syn::Expr> {
        let name = single_ident(expr)?;
        self.binding(&name, self.range(expr).start).map(|(_, init)| init)
    }

    fn is_facing(&self, expr: &syn::Expr) -> bool {
        let expr = strip(expr);
        if single_ident(expr).is_some_and(|name| name == "lr" || name == "facing") || is_lr_call(expr) {
            return true;
        }
        self.local(expr).is_some_and(|init| is_lr_call(strip(init)))
    }

    fn classify(&self, expr: &syn::Expr) -> Classified {
        let mut expr = strip(expr);
        let mut facing_factor = false;
        if let syn::Expr::Binary(binary) = expr {
            if matches!(binary.op, syn::BinOp::Mul(_)) && self.is_facing(&binary.right) {
                facing_factor = true;
                expr = strip(&binary.left);
            }
        }
        let span = self.range(expr);
        let mut referenced_params = Vec::new();
        self.collect_params(expr, &mut referenced_params, &mut Vec::new());

        let value = if let Some(number) = number(expr) {
            SettingValue::Literal(number)
        } else if let Some(name) = const_name(expr) {
            let value = self
                .file_consts
                .and_then(|fc| fc.get(&name))
                .or_else(|| self.consts.get(&name))
                .and_then(|def| def.number());
            SettingValue::Const { name, value }
        } else if let Some((block, field)) =
            param_call(expr).or_else(|| self.local(expr).and_then(|init| param_call(strip(init))))
        {
            SettingValue::Param { block, field }
        } else {
            SettingValue::Computed(computed_text(self.source(expr), &referenced_params))
        };
        Classified { value, span, facing_factor, referenced_params }
    }

    /// True when `expr` reads the current vertical velocity (`KineticModule::get_sum_speed_y`,
    /// `KineticModule::get_speed_y`, or a local `let` bound to one or to a `*_VY` work float).
    fn is_current_vy(&self, expr: &syn::Expr) -> bool {
        let expr = strip(expr);
        if let syn::Expr::Call(call) = expr {
            if let Some(path) = call_path(call) {
                match path.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
                    ["KineticModule", "get_sum_speed_y" | "get_speed_y"] => return true,
                    ["WorkModule", "get_float"] if call.args.len() == 2 => {
                        if let Some(name) = const_name(&call.args[1]) {
                            if name.ends_with("_VY") || name.ends_with("_SPEED_Y") {
                                return true;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(init) = self.local(expr) {
            return self.is_current_vy(init);
        }
        false
    }

    /// Extract the gravity decrement `G` from a per-frame `set_speed` Y argument such as
    /// `current_vy - ARC_GRAVITY` or `if reflected { BOUNCE } else { current_vy - ARC_GRAVITY }`
    /// (directly or through a local `let new_vy = ...;`).
    fn per_frame_gravity<'a>(&'a self, expr: &'a syn::Expr) -> Option<&'a syn::Expr> {
        let expr = strip(expr);
        if let Some(init) = self.local(expr) {
            return self.per_frame_gravity(init);
        }
        match expr {
            syn::Expr::Binary(bin)
                if matches!(bin.op, syn::BinOp::Sub(_)) && self.is_current_vy(&bin.left) =>
            {
                Some(strip(&bin.right))
            }
            syn::Expr::If(if_expr) => {
                // Check `else` branch first because `if reflected { bounce } else { vy - g }`
                // keeps normal flight in `else`.
                let else_match = if_expr
                    .else_branch
                    .as_ref()
                    .and_then(|(_, branch)| self.per_frame_gravity(branch));
                else_match.or_else(|| self.block_tail_gravity(&if_expr.then_branch))
            }
            syn::Expr::Block(block) => self.block_tail_gravity(&block.block),
            _ => None,
        }
    }

    fn block_tail_gravity<'a>(&'a self, block: &'a syn::Block) -> Option<&'a syn::Expr> {
        match block.stmts.last()? {
            syn::Stmt::Expr(expr, _) => self.per_frame_gravity(expr),
            _ => None,
        }
    }

    /// Every `get_param_*` `expr` reads, following local names to the `let`s that bind them.
    fn collect_params(&self, expr: &syn::Expr, found: &mut Vec<(String, String)>, followed: &mut Vec<usize>) {
        struct Reads<'c, 'a> {
            ctx: &'c Ctx<'a>,
            found: &'c mut Vec<(String, String)>,
            followed: &'c mut Vec<usize>,
        }
        impl<'ast> syn::visit::Visit<'ast> for Reads<'_, '_> {
            fn visit_expr(&mut self, expr: &'ast syn::Expr) {
                if let Some(pair) = param_call(expr) {
                    if !self.found.contains(&pair) {
                        self.found.push(pair);
                    }
                    return;
                }
                if let Some(name) = single_ident(expr) {
                    let at = self.ctx.range(expr).start;
                    if let Some((start, init)) = self.ctx.binding(&name, at) {
                        if !self.followed.contains(&start) {
                            self.followed.push(start);
                            self.visit_expr(init);
                        }
                    }
                    return;
                }
                syn::visit::visit_expr(self, expr);
            }
        }
        syn::visit::Visit::visit_expr(&mut Reads { ctx: self, found, followed }, expr);
    }
}

fn computed_text(source: &str, params: &[(String, String)]) -> String {
    if params.is_empty() {
        return source.to_string();
    }
    let reads: Vec<String> = params.iter().map(|(b, f)| format!("{b}.{f}")).collect();
    format!("{source} (reads {})", reads.join(", "))
}

/// Parentheses and invisible groups removed.
fn strip(mut expr: &syn::Expr) -> &syn::Expr {
    loop {
        match expr {
            syn::Expr::Paren(inner) => expr = &inner.expr,
            syn::Expr::Group(inner) => expr = &inner.expr,
            _ => return expr,
        }
    }
}

fn single_ident(expr: &syn::Expr) -> Option<String> {
    let syn::Expr::Path(path) = expr else { return None };
    if path.qself.is_some() {
        return None;
    }
    path.path.get_ident().map(|ident| ident.to_string())
}

fn segments(path: &syn::Path) -> Vec<String> {
    path.segments.iter().map(|s| s.ident.to_string()).collect()
}

/// The last two path segments of a call's function, `["Module", "fn"]` or `["fn"]`.
fn call_path(call: &syn::ExprCall) -> Option<Vec<String>> {
    let syn::Expr::Path(path) = strip(&call.func) else { return None };
    let mut segments = segments(&path.path);
    let keep = segments.len().saturating_sub(2);
    Some(segments.split_off(keep))
}

fn is_call_to(call: &syn::ExprCall, module: &str, function: &str) -> bool {
    call_path(call).is_some_and(|p| p.len() == 2 && p[0] == module && p[1] == function)
}

fn is_lr_call(expr: &syn::Expr) -> bool {
    matches!(expr, syn::Expr::Call(call) if is_call_to(call, "PostureModule", "lr"))
}

fn number(expr: &syn::Expr) -> Option<f64> {
    match strip(expr) {
        syn::Expr::Lit(lit) => match &lit.lit {
            syn::Lit::Float(f) => f.base10_digits().parse().ok(),
            syn::Lit::Int(i) => i.base10_digits().parse().ok(),
            _ => None,
        },
        syn::Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Neg(_)) => number(&unary.expr).map(|n| -n),
        _ => None,
    }
}

fn is_const_case(name: &str) -> bool {
    name.chars().any(|c| c.is_ascii_uppercase())
        && name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// `NAME`, `path::NAME`, `*NAME` (a lua const) or any of them cast with `as`.
fn const_name(expr: &syn::Expr) -> Option<String> {
    match strip(expr) {
        syn::Expr::Path(path) if path.qself.is_none() => {
            let name = path.path.segments.last()?.ident.to_string();
            is_const_case(&name).then_some(name)
        }
        syn::Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Deref(_)) => const_name(&unary.expr),
        syn::Expr::Cast(cast) => const_name(&cast.expr),
        _ => None,
    }
}

/// `(block, field)` of `WorkModule::get_param_float|int(acc, hash40("block"), hash40("field"))`.
fn param_call(expr: &syn::Expr) -> Option<(String, String)> {
    let syn::Expr::Call(call) = strip(expr) else { return None };
    let is_param = is_call_to(call, "WorkModule", "get_param_float") || is_call_to(call, "WorkModule", "get_param_int");
    if !is_param || call.args.len() != 3 {
        return None;
    }
    let hash = |expr: &syn::Expr| -> Option<String> {
        let syn::Expr::Call(call) = strip(expr) else { return None };
        if call_path(call)? != ["hash40"] || call.args.len() != 1 {
            return None;
        }
        match strip(&call.args[0]) {
            syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => Some(s.value()),
            _ => None,
        }
    };
    Some((hash(&call.args[1])?, hash(&call.args[2])?))
}

/// The value a `push_lua_stack` argument pushes: `&mut L2CValue::new_num(v)` gives `v`.
fn pushed_value(expr: &syn::Expr) -> &syn::Expr {
    let mut expr = strip(expr);
    if let syn::Expr::Reference(reference) = expr {
        expr = strip(&reference.expr);
    }
    if let syn::Expr::Call(call) = expr {
        let constructor = call_path(call).and_then(|p| p.last().cloned()).unwrap_or_default();
        if constructor.starts_with("new_") && call.args.len() == 1 {
            return strip(&call.args[0]);
        }
    }
    expr
}

/// Which value after the reserve id sets which field, for the `sv_kinetic_energy` ops this reader
/// models. `None` for any other op, or an arity the op is not read with.
fn kinetic_fields(op: &str, after_reserve: usize) -> Option<Vec<(usize, SettingField)>> {
    use SettingField::*;
    let pair = |x, y| (2, vec![(0, x), (1, y)]);
    let (needed, fields) = match op {
        "set_speed" => pair(SpeedX, SpeedY),
        "set_accel" => pair(AccelX, AccelY),
        "set_stable_speed" => pair(StableSpeedX, StableSpeedY),
        "set_limit_speed" => pair(MaxSpeedX, MaxSpeedY),
        "set_brake" => pair(BrakeX, BrakeY),
        "set_speed_mul" => (1, vec![(0, SpeedMul)]),
        // `(reset_energy, agent, ID, RESET_TYPE, x, y, z, ..)`: the trailing values vary.
        "reset_energy" if after_reserve >= 3 => return Some(vec![(1, SpeedX), (2, SpeedY)]),
        _ => return None,
    };
    (after_reserve == needed).then_some(fields)
}

struct Scanner<'a, 'o> {
    ctx: Ctx<'a>,
    file: &'a Path,
    base: usize,
    status: Status<'a>,
    /// Values pushed since the last `clear_lua_stack` or `sv_kinetic_energy::op(..)` call.
    lua_stack: Vec<syn::Expr>,
    out: &'o mut Vec<Setting>,
    called: &'o mut Vec<String>,
}

impl Scanner<'_, '_> {
    fn record(&mut self, field: SettingField, classified: Classified) {
        self.out.push(Setting {
            field,
            value: classified.value,
            status: self.status.kind.to_string(),
            phase: self.status.phase.to_string(),
            file: self.file.to_path_buf(),
            span: classified.span.start + self.base..classified.span.end + self.base,
            facing_factor: classified.facing_factor,
            referenced_params: classified.referenced_params,
        });
    }

    fn value(&mut self, field: SettingField, expr: &syn::Expr) {
        let classified = self.ctx.classify(expr);
        self.record(field, classified);
    }

    /// A whole call kept as text, under "Other movement".
    fn other(&mut self, whole: &impl syn::spanned::Spanned, args: &[syn::Expr]) {
        let mut referenced_params = Vec::new();
        for arg in args {
            self.ctx.collect_params(arg, &mut referenced_params, &mut Vec::new());
        }
        let classified = Classified {
            value: SettingValue::Computed(computed_text(self.ctx.source(whole), &referenced_params)),
            span: self.ctx.range(whole),
            facing_factor: false,
            referenced_params,
        };
        self.record(SettingField::OtherMovement, classified);
    }

    /// One kinetic call. `after_reserve` holds the values after its reserve id, `all` every
    /// argument (kept with the text when the op is not modelled).
    fn kinetic(
        &mut self,
        op: &str,
        after_reserve: &[syn::Expr],
        whole: &impl syn::spanned::Spanned,
        all: &[syn::Expr],
    ) {
        let Some(fields) = kinetic_fields(op, after_reserve.len()) else {
            self.other(whole, all);
            return;
        };
        for (index, field) in fields {
            let expr = strip(&after_reserve[index]);
            match expr {
                // A negated y acceleration pulls the article down: gravity, by its magnitude.
                syn::Expr::Unary(unary) if field == SettingField::AccelY && matches!(unary.op, syn::UnOp::Neg(_)) => {
                    self.value(SettingField::Gravity, &unary.expr)
                }
                // A per-frame status loop integrating gravity manually:
                // `let new_vy = if ... { ... } else { current_vy - ARC_GRAVITY };`
                // `sv_kinetic_energy!(set_speed, weapon, RESERVE, vx, new_vy);`
                _ if field == SettingField::SpeedY && op == "set_speed" => {
                    if let Some(classified) = self
                        .ctx
                        .per_frame_gravity(expr)
                        .map(|grav_expr| self.ctx.classify(grav_expr))
                    {
                        self.record(SettingField::Gravity, classified);
                    }
                    self.value(field, expr);
                }
                _ => self.value(field, expr),
            }
        }
    }
}

impl<'ast> syn::visit::Visit<'ast> for Scanner<'_, '_> {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let Some(path) = call_path(call) {
            let args: Vec<&syn::Expr> = call.args.iter().collect();
            let is_life = |expr: &syn::Expr| {
                const_name(expr).is_some_and(|name| {
                    name == "WEAPON_INSTANCE_WORK_ID_INT_LIFE" || name == "WEAPON_INSTANCE_WORK_ID_INT_INIT_LIFE"
                })
            };
            match path.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
                ["ModelModule", "set_scale"] if args.len() == 2 => self.value(SettingField::Scale, args[1]),
                ["PostureModule", "set_scale"] if args.len() == 3 => self.value(SettingField::Scale, args[1]),
                ["AttackModule", "set_power_mul"] if args.len() == 2 => self.value(SettingField::PowerMul, args[1]),
                ["WorkModule", "set_int"] if args.len() == 3 && is_life(args[2]) => {
                    self.value(SettingField::Life, args[1])
                }
                ["sv_kinetic_energy", op] => {
                    let op = op.to_string();
                    let stack = std::mem::take(&mut self.lua_stack);
                    match stack.split_first() {
                        Some((_reserve, rest)) => self.kinetic(&op, rest, call, &stack),
                        None => self.other(call, &[]),
                    }
                }
                _ => {}
            }
        }
        syn::visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        match call.method.to_string().as_str() {
            "push_lua_stack" if call.args.len() == 1 => {
                self.lua_stack.push(pushed_value(&call.args[0]).clone());
            }
            "clear_lua_stack" => self.lua_stack.clear(),
            "set_life" if call.args.len() == 1 => self.value(SettingField::Life, &call.args[0]),
            _ => {}
        }
        syn::visit::visit_expr_method_call(self, call);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if mac.path.segments.last().is_some_and(|s| s.ident == "lua_args") {
            // `lua_args!(agent, a, b, ..)` pushes `a, b, ..` like `push_lua_stack` does.
            use syn::parse::Parser;
            let parser = syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated;
            if let Ok(args) = parser.parse2(mac.tokens.clone()) {
                self.lua_stack.extend(args.into_iter().skip(1));
            }
        }
        if mac.path.segments.last().is_some_and(|s| s.ident == "sv_kinetic_energy") {
            use syn::parse::Parser;
            let parser = syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated;
            match parser.parse2(mac.tokens.clone()) {
                Ok(args) => {
                    let args: Vec<syn::Expr> = args.into_iter().collect();
                    let op = args.first().and_then(single_ident).unwrap_or_default();
                    // `(op, agent, RESERVE, ..)`. Ops without a reserve (`controller_*`) and any
                    // op not modelled are kept whole.
                    match args.get(3..) {
                        Some(rest) if !op.is_empty() => self.kinetic(&op, rest, mac, &args),
                        _ => self.other(mac, &args),
                    }
                }
                Err(_) => self.other(mac, &[]),
            }
        }
        syn::visit::visit_macro(self, mac);
    }

    fn visit_expr_path(&mut self, path: &'ast syn::ExprPath) {
        // A name that is not a local may be a project function: a helper, or the status loop a
        // `fast_shift` hands off to.
        if path.qself.is_none() {
            let segments = segments(&path.path);
            let relative = segments[..segments.len() - 1]
                .iter()
                .all(|s| matches!(s.as_str(), "self" | "super" | "crate"));
            let name = segments.last().cloned().unwrap_or_default();
            let at = self.ctx.range(path).start;
            if relative
                && !is_const_case(&name)
                && self.ctx.binding(&name, at).is_none()
                && !self.called.contains(&name)
            {
                self.called.push(name);
            }
        }
        syn::visit::visit_expr_path(self, path);
    }
}

pub use write::{plan_setting_write, SettingWrite};

#[cfg(test)]
mod tests {
    use super::*;

    const SHYGUY_FIREBALL: &str = r#"const FIREBALL_TRAVEL_SPEED: f32 = 2.6;
const FIREBALL_MAX_LIFETIME: i32 = 180;
const FIREBALL_SCALE: f32 = 1.4;

pub(super) unsafe extern "C" fn fireball_main(weapon: &mut L2CWeaponCommon) -> L2CValue {
    ModelModule::set_scale(weapon.module_accessor, FIREBALL_SCALE);
    let facing = spawn_at_owner(weapon, FIREBALL_SPAWN_OFFSET_X, FIREBALL_SPAWN_OFFSET_Y);
    KineticModule::change_kinetic(weapon.module_accessor, *WEAPON_KINETIC_TYPE_NORMAL);
    sv_kinetic_energy!(set_speed, weapon, WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL, FIREBALL_TRAVEL_SPEED * facing, 0.0);
    weapon.set_life(FIREBALL_MAX_LIFETIME);
    weapon.fast_shift_status(fireball_main_loop)
}
"#;

    const SHYGUY_MAIN_LOOP: &str = r#"unsafe extern "C" fn fireball_main_loop(weapon: &mut L2CWeaponCommon) -> L2CValue {
    let facing = PostureModule::lr(weapon.module_accessor);
    sv_kinetic_energy!(set_speed, weapon, WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL, FIREBALL_TRAVEL_SPEED * facing, 0.0);
    deeper_helper(weapon);
    0.into()
}
"#;

    const SHYGUY_DEEPER: &str = r#"unsafe fn deeper_helper(weapon: &mut L2CWeaponCommon) {
    ModelModule::set_scale(weapon.module_accessor, 9.0);
}
"#;

    const LATIOS_SWORDBEAM: &str = r#"unsafe extern "C" fn link_swordbeam_fly_main(weapon: &mut L2CWeaponCommon) -> L2CValue {
    let charge_rate = WorkModule::get_float(weapon.module_accessor, *WEAPON_LINK_SWORDBEAM_INSTANCE_WORK_ID_FLOAT_CHARGE_RATE);
    let min_life = WorkModule::get_param_int(weapon.module_accessor, hash40("param_swordbeam"), hash40("min_life")); // 32
    let max_life = WorkModule::get_param_int(weapon.module_accessor, hash40("param_swordbeam"), hash40("max_life")); // 40
    let lerp = weapon.lerp(min_life.into(), max_life.into(), charge_rate.into()).get_i32();
    WorkModule::set_int(weapon.module_accessor, lerp, *WEAPON_INSTANCE_WORK_ID_INT_LIFE);
    WorkModule::set_int(weapon.module_accessor, lerp, *WEAPON_INSTANCE_WORK_ID_INT_INIT_LIFE);

    let min_speed_x = WorkModule::get_param_float(weapon.module_accessor, hash40("param_swordbeam"), hash40("min_speed_x")); // 2.4
    let max_speed_x = WorkModule::get_param_float(weapon.module_accessor, hash40("param_swordbeam"), hash40("max_speed_x")); // 3.4
    let lerp2 = weapon.lerp(min_speed_x.into(), max_speed_x.into(), charge_rate.into()).get_f32();
    let deccel_x = WorkModule::get_param_float(weapon.module_accessor, hash40("param_swordbeam"), hash40("deccel_x")); // 0.01
    let facing = PostureModule::lr(weapon.module_accessor);
    let speed_x_final = lerp2 * facing;
    let deccel_x_final = deccel_x * -1.0;

    weapon.agent.clear_lua_stack();
    weapon.agent.push_lua_stack(&mut L2CValue::new_int(*WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL as u64));
    weapon.agent.push_lua_stack(&mut L2CValue::new_num(speed_x_final));
    weapon.agent.push_lua_stack(&mut L2CValue::new_num(0.0));
    sv_kinetic_energy::set_speed(weapon.lua_state_agent);

    weapon.agent.clear_lua_stack();
    weapon.agent.push_lua_stack(&mut L2CValue::new_int(*WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL as u64));
    weapon.agent.push_lua_stack(&mut L2CValue::new_num(deccel_x_final));
    weapon.agent.push_lua_stack(&mut L2CValue::new_num(0.0));
    sv_kinetic_energy::set_accel(weapon.lua_state_agent);
    0.into()
}
"#;

    const KINGBOO_SHOOT: &str = r#"const FIREBALL_SPEED_X:       f32 = 0.5;
const FIREBALL_SPEED_X_BOOST: f32 = 0.3;
const FIREBALL_SPEED_Y:       f32 = 2.1;
const FIREBALL_SPEED_Y_BOOST: f32 = 2.2;
const FIREBALL_ACCEL_Y:       f32 = 0.08;
const FIREBALL_ACCEL_Y_BOOST: f32 = 0.08;

unsafe extern "C" fn kingteresa_fireball_shoot_status_main(weapon: &mut L2CWeaponCommon) -> L2CValue {
    let owner_boma = article_get_owner_boma(weapon.module_accessor);
    let lr = PostureModule::lr(owner_boma);
    let hold_rate = WorkModule::get_float(owner_boma, *FIGHTER_STATUS_WORK_ID_FLOAT_RESERVE_HOLD_RATE);
    let speed_x = FIREBALL_SPEED_X + (FIREBALL_SPEED_X_BOOST * hold_rate);
    let speed_y = FIREBALL_SPEED_Y + (FIREBALL_SPEED_Y_BOOST * hold_rate);
    let accel_y = FIREBALL_ACCEL_Y + (FIREBALL_ACCEL_Y_BOOST * hold_rate);
    KineticModule::enable_energy(weapon.module_accessor, *WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL);
    sv_kinetic_energy!(set_speed, weapon, WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL, speed_x * lr, speed_y);
    sv_kinetic_energy!(set_accel, weapon, WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL, 0.0, -accel_y);
    0.into()
}
"#;

    const KINGBOO_EXPLODE: &str = r#"unsafe extern "C" fn kingteresa_fireball_explode_status_main(weapon: &mut L2CWeaponCommon) -> L2CValue {
    KineticModule::clear_speed_all(weapon.module_accessor);
    WorkModule::set_int(weapon.module_accessor, 30, *WEAPON_INSTANCE_WORK_ID_INT_INIT_LIFE);
    WorkModule::set_int(weapon.module_accessor, 30, *WEAPON_INSTANCE_WORK_ID_INT_LIFE);
    0.into()
}
"#;

    const KINGBOO_INIT: &str = r#"unsafe extern "C" fn kingteresa_fireball_shoot_status_init(weapon: &mut L2CWeaponCommon) -> L2CValue {
    let owner_boma = article_get_owner_boma(weapon.module_accessor);
    let hold_rate = WorkModule::get_float(owner_boma, *FIGHTER_STATUS_WORK_ID_FLOAT_RESERVE_HOLD_RATE);
    let charge_boost = 0.4 * hold_rate;
    AttackModule::set_power_mul(weapon.module_accessor, charge_boost + 1.0);
    0.into()
}
"#;

    /// The function in `text` named `name`, from its line start through its closing brace.
    fn fn_span(text: &str, name: &str) -> Range<usize> {
        let at = text.find(&format!("fn {name}(")).expect("fixture names the function");
        let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
        let close = text[start..].find("\n}").expect("closing brace") + start + 2;
        start..close
    }

    fn site(file: &str, text: &str, name: &str) -> StatusSite {
        StatusSite {
            kind_expr: format!("*KIND_{}", name.to_uppercase()),
            phase: "Main".into(),
            function: name.into(),
            file: PathBuf::from(file),
            span: fn_span(text, name),
        }
    }

    /// Reads `name` in `text` as a Main status, following no helpers.
    fn read(text: &str, name: &str) -> Vec<Setting> {
        read_with(text, name, &|_, _| None)
    }

    fn read_with(
        text: &str,
        name: &str,
        helpers: &dyn Fn(&Path, &str) -> Option<(PathBuf, Range<usize>, String)>,
    ) -> Vec<Setting> {
        let consts = ProjectConsts::scan(&[(PathBuf::from("f.rs"), text.to_string())]);
        let site = site("f.rs", text, name);
        let body = text[site.span.clone()].to_string();
        read_settings(&[(&site, body)], &consts, helpers)
    }

    fn only(settings: &[Setting], field: SettingField) -> &Setting {
        let found: Vec<&Setting> = settings.iter().filter(|s| s.field == field).collect();
        assert_eq!(found.len(), 1, "{field:?} in {settings:#?}");
        found[0]
    }

    /// Shy-Guy-Moveset · plugin/src/shyguy/articles/trampoline/status/fireball.rs:18
    #[test]
    fn scale_const_shyguy() {
        let s = read(SHYGUY_FIREBALL, "fireball_main");
        let scale = only(&s, SettingField::Scale);
        assert_eq!(
            scale.value,
            SettingValue::Const { name: "FIREBALL_SCALE".into(), value: Some(1.4) }
        );
        assert!(!scale.facing_factor);
        assert_eq!(scale.status, "*KIND_FIREBALL_MAIN");
        assert_eq!(scale.phase, "Main");
    }

    /// Shy-Guy-Moveset · plugin/src/shyguy/articles/trampoline/status/fireball.rs:38
    #[test]
    fn speed_const_with_facing_shyguy() {
        let s = read(SHYGUY_FIREBALL, "fireball_main");
        let x = only(&s, SettingField::SpeedX);
        assert_eq!(
            x.value,
            SettingValue::Const { name: "FIREBALL_TRAVEL_SPEED".into(), value: Some(2.6) }
        );
        assert!(x.facing_factor);
        assert_eq!(&SHYGUY_FIREBALL[x.span.clone()], "FIREBALL_TRAVEL_SPEED");
        let y = only(&s, SettingField::SpeedY);
        assert_eq!(y.value, SettingValue::Literal(0.0));
        assert!(!y.facing_factor);
    }

    /// Shy-Guy-Moveset · plugin/src/shyguy/articles/trampoline/status/fireball.rs:41 (`set_life`).
    #[test]
    fn set_life_const_shyguy() {
        let s = read(SHYGUY_FIREBALL, "fireball_main");
        let life = only(&s, SettingField::Life);
        assert_eq!(
            life.value,
            SettingValue::Const { name: "FIREBALL_MAX_LIFETIME".into(), value: Some(180.0) }
        );
    }

    /// Latios-Moveset · src/ground_smash/mod.rs:39-43
    #[test]
    fn life_param_via_let_latios() {
        let s = read(LATIOS_SWORDBEAM, "link_swordbeam_fly_main");
        let lives: Vec<&Setting> = s.iter().filter(|s| s.field == SettingField::Life).collect();
        // `INT_LIFE` and `INT_INIT_LIFE` are both set.
        assert_eq!(lives.len(), 2, "{s:#?}");
        for life in lives {
            let SettingValue::Computed(text) = &life.value else {
                panic!("a lerp of two params is computed: {life:?}");
            };
            assert!(text.contains("param_swordbeam.min_life"), "{text}");
            assert!(text.contains("param_swordbeam.max_life"), "{text}");
            assert!(life.referenced_params.contains(&("param_swordbeam".into(), "min_life".into())));
            assert!(life.referenced_params.contains(&("param_swordbeam".into(), "max_life".into())));
            assert_eq!(&LATIOS_SWORDBEAM[life.span.clone()], "lerp");
        }
    }

    /// A `let` bound to one `get_param_*` call is the param itself.
    #[test]
    fn param_via_let_is_param() {
        let text = r#"unsafe extern "C" fn main(weapon: &mut L2CWeaponCommon) -> L2CValue {
    let life = WorkModule::get_param_int(weapon.module_accessor, hash40("param_fireball"), hash40("life"));
    WorkModule::set_int(weapon.module_accessor, life, *WEAPON_INSTANCE_WORK_ID_INT_LIFE);
    AttackModule::set_power_mul(weapon.module_accessor, WorkModule::get_param_float(weapon.module_accessor, hash40("param_fireball"), hash40("power_mul")));
    0.into()
}
"#;
        let s = read(text, "main");
        let life = only(&s, SettingField::Life);
        assert_eq!(
            life.value,
            SettingValue::Param { block: "param_fireball".into(), field: "life".into() }
        );
        assert_eq!(life.referenced_params, [("param_fireball".to_string(), "life".to_string())]);
        let power = only(&s, SettingField::PowerMul);
        assert_eq!(
            power.value,
            SettingValue::Param { block: "param_fireball".into(), field: "power_mul".into() }
        );
    }

    /// King-Boo-Moveset · src/kingteresa/weapon/fireball/status.rs:85-86
    #[test]
    fn life_literal_kingboo() {
        let s = read(KINGBOO_EXPLODE, "kingteresa_fireball_explode_status_main");
        let lives: Vec<&Setting> = s.iter().filter(|s| s.field == SettingField::Life).collect();
        assert_eq!(lives.len(), 2);
        for life in lives {
            assert_eq!(life.value, SettingValue::Literal(30.0));
            assert_eq!(&KINGBOO_EXPLODE[life.span.clone()], "30");
        }
    }

    /// King-Boo-Moveset · src/kingteresa/weapon/fireball/status.rs:49-50
    #[test]
    fn accel_negative_is_gravity_kingboo() {
        let s = read(KINGBOO_SHOOT, "kingteresa_fireball_shoot_status_main");
        let g = only(&s, SettingField::Gravity);
        assert_eq!(g.value, SettingValue::Computed("accel_y".into()));
        assert_eq!(&KINGBOO_SHOOT[g.span.clone()], "accel_y");
        assert!(g.referenced_params.is_empty());
        assert_eq!(only(&s, SettingField::AccelX).value, SettingValue::Literal(0.0));
        assert!(s.iter().all(|s| s.field != SettingField::AccelY));

        let x = only(&s, SettingField::SpeedX);
        assert_eq!(x.value, SettingValue::Computed("speed_x".into()));
        assert!(x.facing_factor, "`* lr` is the facing");
        assert_eq!(only(&s, SettingField::SpeedY).value, SettingValue::Computed("speed_y".into()));
    }

    /// King-Boo-Moveset · src/kingteresa/weapon/fireball/status.rs:31
    #[test]
    fn power_mul_computed_kingboo() {
        let s = read(KINGBOO_INIT, "kingteresa_fireball_shoot_status_init");
        let p = only(&s, SettingField::PowerMul);
        assert_eq!(p.value, SettingValue::Computed("charge_boost + 1.0".into()));
    }

    /// Latios-Moveset · src/ground_smash/mod.rs:54-64
    #[test]
    fn lua_stack_speed_latios() {
        let s = read(LATIOS_SWORDBEAM, "link_swordbeam_fly_main");
        let x = only(&s, SettingField::SpeedX);
        let SettingValue::Computed(text) = &x.value else { panic!("{x:?}") };
        assert!(text.starts_with("speed_x_final"), "{text}");
        assert!(x.referenced_params.contains(&("param_swordbeam".into(), "min_speed_x".into())));
        assert_eq!(&LATIOS_SWORDBEAM[x.span.clone()], "speed_x_final");
        let y = only(&s, SettingField::SpeedY);
        assert_eq!(y.value, SettingValue::Literal(0.0));

        let ax = only(&s, SettingField::AccelX);
        assert!(matches!(ax.value, SettingValue::Computed(_)), "{ax:?}");
        assert_eq!(only(&s, SettingField::AccelY).value, SettingValue::Literal(0.0));
    }

    #[test]
    fn span_points_at_value() {
        let text = SHYGUY_FIREBALL;
        let s = read(text, "fireball_main");
        let scale = only(&s, SettingField::Scale);
        assert_eq!(&text[scale.span.clone()], "FIREBALL_SCALE");
        assert_eq!(scale.file, PathBuf::from("f.rs"));
    }

    /// Spans are byte offsets into the file, through CRLF line ends and multi-byte characters.
    #[test]
    fn spans_survive_crlf_and_non_ascii() {
        let text = SHYGUY_FIREBALL
            .replace(
                "    ModelModule::set_scale",
                "    /* ══ größe ══ */ ModelModule::set_scale",
            )
            .replace('\n', "\r\n");
        let text = format!("// ══ TUNING ══\r\n{text}");
        let s = read(&text, "fireball_main");
        let scale = only(&s, SettingField::Scale);
        assert_eq!(&text[scale.span.clone()], "FIREBALL_SCALE");
        let x = only(&s, SettingField::SpeedX);
        assert_eq!(&text[x.span.clone()], "FIREBALL_TRAVEL_SPEED");
        let y = only(&s, SettingField::SpeedY);
        assert_eq!(&text[y.span.clone()], "0.0");
    }

    /// Shy-Guy-Moveset · plugin/src/shyguy/articles/trampoline/status/fireball.rs:43,59 — the main
    /// shifts to `fireball_main_loop`, which sets the speed again. That one is followed; a call
    /// inside it is not.
    #[test]
    fn follows_a_project_function_one_level() {
        let loop_file = PathBuf::from("loop.rs");
        let helpers = |_: &Path, name: &str| match name {
            "fireball_main_loop" => Some((
                loop_file.clone(),
                fn_span(SHYGUY_MAIN_LOOP, name),
                SHYGUY_MAIN_LOOP[fn_span(SHYGUY_MAIN_LOOP, name)].to_string(),
            )),
            "deeper_helper" => Some((
                PathBuf::from("deeper.rs"),
                fn_span(SHYGUY_DEEPER, name),
                SHYGUY_DEEPER[fn_span(SHYGUY_DEEPER, name)].to_string(),
            )),
            _ => None,
        };
        let s = read_with(SHYGUY_FIREBALL, "fireball_main", &helpers);
        let xs: Vec<&Setting> = s.iter().filter(|s| s.field == SettingField::SpeedX).collect();
        assert_eq!(xs.len(), 2, "{s:#?}");
        let from_loop = xs.iter().find(|s| s.file == loop_file).expect("helper read");
        assert_eq!(&SHYGUY_MAIN_LOOP[from_loop.span.clone()], "FIREBALL_TRAVEL_SPEED");
        assert!(from_loop.facing_factor);
        assert_eq!(from_loop.status, "*KIND_FIREBALL_MAIN");
        // Only one level: `deeper_helper`'s scale is not read.
        assert!(s.iter().all(|s| s.file != PathBuf::from("deeper.rs")), "{s:#?}");
    }

    /// Ops the reader does not model are kept, under "Other movement", never dropped.
    #[test]
    fn unknown_kinetic_op_is_other_movement() {
        let text = r#"unsafe extern "C" fn main(weapon: &mut L2CWeaponCommon) -> L2CValue {
    sv_kinetic_energy!(controller_set_accel_x_mul, weapon, 0.5);
    sv_kinetic_energy!(set_speed_mul, weapon, WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL, 0.8);
    sv_kinetic_energy!(reset_energy, weapon, WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL, ENERGY_STOP_RESET_TYPE_AIR, 1.5, 2.5, 0.0, 0.0, 0.0);
    sv_kinetic_energy!(set_limit_speed, weapon, WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL, 3.0, 4.0);
    0.into()
}
"#;
        let s = read(text, "main");
        let other = only(&s, SettingField::OtherMovement);
        let SettingValue::Computed(t) = &other.value else { panic!("{other:?}") };
        assert!(t.contains("controller_set_accel_x_mul"), "{t}");
        assert_eq!(&text[other.span.clone()], t.as_str());
        assert_eq!(only(&s, SettingField::SpeedMul).value, SettingValue::Literal(0.8));
        assert_eq!(only(&s, SettingField::SpeedX).value, SettingValue::Literal(1.5));
        assert_eq!(only(&s, SettingField::SpeedY).value, SettingValue::Literal(2.5));
        assert_eq!(only(&s, SettingField::MaxSpeedX).value, SettingValue::Literal(3.0));
        assert_eq!(only(&s, SettingField::MaxSpeedY).value, SettingValue::Literal(4.0));
    }

    /// A negative literal y accel is a gravity of its magnitude; a negative literal elsewhere stays
    /// signed.
    #[test]
    fn negative_literals() {
        let text = r#"unsafe extern "C" fn main(weapon: &mut L2CWeaponCommon) -> L2CValue {
    sv_kinetic_energy!(set_accel, weapon, WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL, -0.01, -0.05);
    PostureModule::set_scale(weapon.module_accessor, 1.2, false);
    0.into()
}
"#;
        let s = read(text, "main");
        let g = only(&s, SettingField::Gravity);
        assert_eq!(g.value, SettingValue::Literal(0.05));
        assert_eq!(&text[g.span.clone()], "0.05");
        let ax = only(&s, SettingField::AccelX);
        assert_eq!(ax.value, SettingValue::Literal(-0.01));
        assert_eq!(&text[ax.span.clone()], "-0.01");
        assert_eq!(only(&s, SettingField::Scale).value, SettingValue::Literal(1.2));
    }

    /// A shadowing `let` reads the binding before it, so the param origin survives.
    #[test]
    fn shadowing_let_reads_the_earlier_binding() {
        let text = r#"unsafe extern "C" fn main(weapon: &mut L2CWeaponCommon) -> L2CValue {
    let x = WorkModule::get_param_float(weapon.module_accessor, hash40("b"), hash40("f"));
    let x = x * 2.0;
    sv_kinetic_energy!(set_speed, weapon, WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL, x, 0.0);
    0.into()
}
"#;
        let s = read(text, "main");
        let speed = only(&s, SettingField::SpeedX);
        assert_eq!(speed.referenced_params, [("b".to_string(), "f".to_string())]);
        let SettingValue::Computed(t) = &speed.value else { panic!("{speed:?}") };
        assert!(t.contains("b.f"), "{t}");
    }

    /// A site span that starts mid-line, after the previous item's closing brace, still parses.
    #[test]
    fn body_starting_mid_line_parses() {
        let text = "fn other() {\n} unsafe extern \"C\" fn main(weapon: &mut L2CWeaponCommon) -> L2CValue {\n    ModelModule::set_scale(weapon.module_accessor, 1.5);\n    0.into()\n}\n";
        let start = text.find("} unsafe").unwrap();
        let site = StatusSite {
            kind_expr: "*KIND_MAIN".into(),
            phase: "Main".into(),
            function: "main".into(),
            file: PathBuf::from("f.rs"),
            span: start..text.len(),
        };
        let s = read_settings(&[(&site, text[site.span.clone()].to_string())], &ProjectConsts::default(), &|_, _| None);
        let scale = only(&s, SettingField::Scale);
        assert_eq!(scale.value, SettingValue::Literal(1.5));
        assert_eq!(&text[scale.span.clone()], "1.5");
    }

    /// A body that cannot be read is reported, never dropped.
    #[test]
    fn unreadable_body_is_reported() {
        let text = "unsafe extern \"C\" fn main(weapon: &mut L2CWeaponCommon) -> L2CValue {\n    ModelModule::set_scale(weapon.module_accessor, 1.5\n}\n";
        let site = StatusSite {
            kind_expr: "*KIND_MAIN".into(),
            phase: "Main".into(),
            function: "main".into(),
            file: PathBuf::from("f.rs"),
            span: 0..text.len(),
        };
        let s = read_settings(&[(&site, text.to_string())], &ProjectConsts::default(), &|_, _| None);
        let other = only(&s, SettingField::OtherMovement);
        assert_eq!(
            other.value,
            SettingValue::Computed("Visionary could not read this status function".into())
        );
        assert_eq!(other.span, 0..text.len());
        assert_eq!(s.len(), 1);
    }

    /// Two modules each define `main_loop`: the helper read is the one the caller's file names.
    #[test]
    fn same_named_helpers_resolve_by_caller_file() {
        let caller = |speed: &str| {
            format!(
                "unsafe extern \"C\" fn main(weapon: &mut L2CWeaponCommon) -> L2CValue {{\n    ModelModule::set_scale(weapon.module_accessor, {speed});\n    weapon.fast_shift_status(main_loop)\n}}\n"
            )
        };
        let loop_body = |v: &str| {
            format!("unsafe extern \"C\" fn main_loop(weapon: &mut L2CWeaponCommon) -> L2CValue {{\n    AttackModule::set_power_mul(weapon.module_accessor, {v});\n    0.into()\n}}\n")
        };
        let (a, b) = (caller("1.0"), caller("2.0"));
        let (loop_a, loop_b) = (loop_body("1.1"), loop_body("2.2"));
        let site_a = StatusSite { file: PathBuf::from("a/status.rs"), ..site("x", &a, "main") };
        let site_b = StatusSite { file: PathBuf::from("b/status.rs"), ..site("x", &b, "main") };
        let helpers = |from: &Path, name: &str| {
            if name != "main_loop" {
                return None;
            }
            let (file, text) = if from == Path::new("a/status.rs") {
                ("a/status.rs", &loop_a)
            } else {
                ("b/status.rs", &loop_b)
            };
            Some((PathBuf::from(file), 0..text.len(), text.clone()))
        };
        let s = read_settings(
            &[(&site_a, a.clone()), (&site_b, b.clone())],
            &ProjectConsts::default(),
            &helpers,
        );
        let powers: Vec<(&Path, &SettingValue)> = s
            .iter()
            .filter(|s| s.field == SettingField::PowerMul)
            .map(|s| (s.file.as_path(), &s.value))
            .collect();
        assert_eq!(
            powers,
            [
                (Path::new("a/status.rs"), &SettingValue::Literal(1.1)),
                (Path::new("b/status.rs"), &SettingValue::Literal(2.2)),
            ]
        );
    }

    /// Potemkin-SL2 · src/moveset/articles/ryusensya/specialn.rs:109-112 (tier 1, smashline2),
    /// adapted: those four lines, verbatim, inside a trimmed `fly_main`. `lua_args!` fills the
    /// lua stack the way `push_lua_stack` does.
    #[test]
    fn lua_args_speed_potemkin() {
        let text = r#"pub unsafe extern "C" fn fly_main(weapon: &mut L2CWeaponCommon) -> L2CValue {
    let posture = PostureModule::lr(weapon.module_accessor);
    weapon.clear_lua_stack();
    lua_args!(weapon,*WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL, 2.0 * posture, 0.0);
    sv_kinetic_energy::set_speed(weapon.lua_state_agent);
    0.into()
}
"#;
        let s = read(text, "fly_main");
        assert!(s.iter().all(|s| s.field != SettingField::OtherMovement), "{s:#?}");
        let x = only(&s, SettingField::SpeedX);
        assert_eq!(x.value, SettingValue::Literal(2.0));
        assert!(x.facing_factor, "`posture` is bound to `PostureModule::lr`");
        let x_at = text.find("2.0 * posture").unwrap();
        assert_eq!(x.span, x_at..x_at + 3);
        let y = only(&s, SettingField::SpeedY);
        assert_eq!(y.value, SettingValue::Literal(0.0));
        let y_at = text.find("0.0);").unwrap();
        assert_eq!(y.span, y_at..y_at + 3);
    }

    /// A broken status holding a well-formed nested function is reported as unreadable; the nested
    /// function is not read in its place.
    #[test]
    fn nested_function_in_broken_body_is_not_read_as_the_status() {
        let text = "unsafe extern \"C\" fn status_main(weapon: &mut L2CWeaponCommon) -> L2CValue {\n    fn inner() { ModelModule::set_scale(acc, 2.0); }\n    let = ;\n    0.into()\n}\n";
        let site = StatusSite {
            kind_expr: "*KIND_MAIN".into(),
            phase: "Main".into(),
            function: "status_main".into(),
            file: PathBuf::from("f.rs"),
            span: 0..text.len(),
        };
        let s = read_settings(&[(&site, text.to_string())], &ProjectConsts::default(), &|_, _| None);
        assert_eq!(s.len(), 1, "{s:#?}");
        assert_eq!(s[0].field, SettingField::OtherMovement);
        assert_eq!(
            s[0].value,
            SettingValue::Computed("Visionary could not read this status function".into())
        );
        assert_eq!(s[0].span, 0..text.len());
        assert!(s.iter().all(|s| s.field != SettingField::Scale));
    }
}

/// Where an edited setting is saved: at the literal in the status code, at a const's definition,
/// or in the param file the value is read from.
pub mod write {
    use std::path::PathBuf;

    use anyhow::{bail, Context, Result};

    use super::{Setting, SettingField, SettingValue};
    use crate::project_consts::ConstEdit;

    #[derive(Debug, Clone, PartialEq)]
    pub enum SettingWrite {
        /// `file`'s whole new text: only the literal's bytes differ.
        Source { file: PathBuf, text: String },
        /// Change the const's definition (through [`crate::project_consts::apply_edits`], which
        /// spells the number the way the definition is written).
        Const(ConstEdit),
        /// Write `variant` of `block.field` in `file`, one of the moveset's own param files
        /// (through [`crate::articles::params::write_param_field`]).
        Param { file: PathBuf, block: String, field: String, variant: usize },
    }

    /// Plan saving `new` for `setting`. `variant` is the list entry a `Param` value is read from (0
    /// for a plain struct). `own` is the moveset's OWN param files (its romfs `vl.prc`/`vl.prcxml`),
    /// lowest first: never the game dump or a reference mod, which a save must not change.
    ///
    /// - A `Literal` has its bytes in the status code replaced, written the way the literal was
    ///   (`30` → `45`, `30.0` → `45.0`, `1.6f32` → `2.0f32`); every other byte of the file is kept.
    /// - A `Const` becomes a [`ConstEdit`] on its definition.
    /// - A `Param` goes to the top-most of the moveset's own files that already has that block,
    ///   variant and field. When none has it, this is an error: the param writer cannot add a
    ///   field, and writing the dump or another mod would not change this moveset.
    /// - A `Computed` value, and anything this cannot write, is an error saying why.
    ///
    /// Nothing is written here: the caller applies the plan.
    pub fn plan_setting_write(setting: &Setting, new: f64, variant: usize, own: &[PathBuf]) -> Result<SettingWrite> {
        if !new.is_finite() {
            bail!("{new} is not a number Visionary can write");
        }
        if setting.field == SettingField::Gravity && new < 0.0 {
            bail!(
                "Gravity is written as a pull downwards (a negated acceleration), so it can't be \
                 negative here; set Acceleration Y in the source instead"
            );
        }
        match &setting.value {
            SettingValue::Computed(text) => bail!(
                "This value is worked out from other values ({text}), so Visionary can't change it \
                 without losing how it is worked out; change it in the source"
            ),
            SettingValue::Literal(old) => {
                let file = &setting.file;
                let mut text =
                    std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
                let Some(written) = text.get(setting.span.clone()).map(str::to_string) else {
                    bail!("{} changed since it was read — rescan and try again", file.display());
                };
                if literal_value(&written).is_none_or(|value| (value - old).abs() > 1e-9) {
                    bail!(
                        "{} changed since it was read (the value there is now `{written}`) — rescan and \
                         try again",
                        file.display()
                    );
                }
                let Some(value) = crate::project_consts::format_literal_like(&written, new) else {
                    bail!(
                        "{} is written as a whole number (`{written}`), so it can't hold {new}",
                        setting.field.label()
                    );
                };
                text.replace_range(setting.span.clone(), &value);
                Ok(SettingWrite::Source { file: file.clone(), text })
            }
            SettingValue::Const { name, value } => {
                if value.is_none() {
                    bail!(
                        "{name} is not a number this project defines, so Visionary can't find where to \
                         change it; change it where it is defined"
                    );
                }
                // Plain digits: `apply_edits` spells them the way the definition is written.
                Ok(SettingWrite::Const(ConstEdit { name: name.clone(), value: new.to_string() }))
            }
            SettingValue::Param { block, field } => {
                for layer in own.iter().rev() {
                    // An own file that cannot be read may be the one the game reads, so choosing one
                    // below it could write a value the game never sees.
                    let has = crate::articles::params::layer_has_field(layer, block, field, variant)
                        .map_err(|error| {
                            anyhow::anyhow!("{error}, so Visionary can't tell which file holds {block}.{field}")
                        })?;
                    if has {
                        return Ok(SettingWrite::Param {
                            file: layer.clone(),
                            block: block.clone(),
                            field: field.clone(),
                            variant,
                        });
                    }
                }
                let entry = if variant == 0 { String::new() } else { format!(" (list entry {variant})") };
                // The file to add it to: the moveset's prcxml when it has one.
                let file = own
                    .iter()
                    .rev()
                    .find(|file| file.extension().is_some_and(|e| e.eq_ignore_ascii_case("prcxml")))
                    .or(own.last());
                match file {
                    Some(file) => bail!(
                        "`{field}`{entry} isn't in {block} in this moveset's own param file yet, so \
                         Visionary can't change it there; add it to {} first",
                        file.display()
                    ),
                    None => bail!(
                        "`{field}`{entry} is read from {block}, but this moveset has no param file of \
                         its own, so Visionary can't change it; add a vl.prcxml with it first"
                    ),
                }
            }
        }
    }

    /// The number a literal's text spells (`30`, `-1.5`, `1.6f32`, `1_000`).
    fn literal_value(text: &str) -> Option<f64> {
        let expr: syn::Expr = syn::parse_str(text.trim()).ok()?;
        super::number(&expr)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::collections::HashMap;
        use std::path::Path;

        const STATUS: &str = "unsafe extern \"C\" fn shot_main(weapon: &mut L2CWeaponCommon) -> L2CValue {\r\n    weapon.set_life(30);\r\n    ModelModule::set_scale(weapon.module_accessor, 1.6f32);\r\n    AttackModule::set_power_mul(weapon.module_accessor, 30.0);\r\n    sv_kinetic_energy!(set_accel, weapon, WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL, 0.0, -0.08);\r\n    0.into()\r\n}\r\n";

        fn setting(file: &Path, text: &str, field: SettingField, value: SettingValue, written: &str) -> Setting {
            let start = text.find(written).expect("fixture holds the value");
            Setting {
                field,
                value,
                status: "*WEAPON_X_STATUS_KIND_SHOOT".into(),
                phase: "Main".into(),
                file: file.to_path_buf(),
                span: start..start + written.len(),
                facing_factor: false,
                referenced_params: Vec::new(),
            }
        }

        fn source_text(write: SettingWrite) -> String {
            match write {
                SettingWrite::Source { text, .. } => text,
                other => panic!("expected a source write, got {other:?}"),
            }
        }

        /// The settings reader's own spans: what it records is what the writer splices.
        #[test]
        fn literal_setting_keeps_style() {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("shot.rs");
            std::fs::write(&file, STATUS).unwrap();
            let life = setting(&file, STATUS, SettingField::Life, SettingValue::Literal(30.0), "30)");
            let life = Setting { span: life.span.start..life.span.end - 1, ..life };
            let scale = setting(&file, STATUS, SettingField::Scale, SettingValue::Literal(1.6), "1.6f32");
            let power = setting(&file, STATUS, SettingField::PowerMul, SettingValue::Literal(30.0), "30.0");
            let gravity = setting(&file, STATUS, SettingField::Gravity, SettingValue::Literal(0.08), "0.08");

            let text = source_text(plan_setting_write(&life, 45.0, 0, &[]).unwrap());
            assert_eq!(text, STATUS.replace("set_life(30)", "set_life(45)"));
            let text = source_text(plan_setting_write(&scale, 2.0, 0, &[]).unwrap());
            assert_eq!(text, STATUS.replace("1.6f32", "2.0f32"));
            let text = source_text(plan_setting_write(&power, 45.0, 0, &[]).unwrap());
            assert_eq!(text, STATUS.replace("30.0", "45.0"));
            // Gravity is the magnitude of a negated acceleration: the `-` stays where it is.
            let text = source_text(plan_setting_write(&gravity, 0.1, 0, &[]).unwrap());
            assert_eq!(text, STATUS.replace("-0.08", "-0.1"));
            assert_eq!(text.matches("\r\n").count(), STATUS.matches("\r\n").count());

            let error = plan_setting_write(&life, 45.5, 0, &[]).unwrap_err().to_string();
            assert!(error.contains("whole number"), "{error}");
            let error = plan_setting_write(&gravity, -0.1, 0, &[]).unwrap_err().to_string();
            assert!(error.contains("negative"), "{error}");
            assert_eq!(std::fs::read_to_string(&file).unwrap(), STATUS, "planning writes nothing");
        }

        /// The spans the reader gives for a real status agree with the writer.
        #[test]
        fn literal_setting_from_the_reader() {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("shot.rs");
            std::fs::write(&file, STATUS).unwrap();
            let consts = crate::project_consts::ProjectConsts::default();
            let site = crate::acmd_src::StatusSite {
                kind_expr: "*WEAPON_X_STATUS_KIND_SHOOT".into(),
                phase: "Main".into(),
                function: "shot_main".into(),
                file: file.clone(),
                span: 0..STATUS.len(),
            };
            let settings = super::super::read_settings(&[(&site, STATUS.to_string())], &consts, &|_, _| None);
            let gravity = settings.iter().find(|s| s.field == SettingField::Gravity).unwrap();
            let text = source_text(plan_setting_write(gravity, 0.5, 0, &[]).unwrap());
            assert_eq!(text, STATUS.replace("-0.08", "-0.5"));
            let life = settings.iter().find(|s| s.field == SettingField::Life).unwrap();
            let text = source_text(plan_setting_write(life, 90.0, 0, &[]).unwrap());
            assert_eq!(text, STATUS.replace("set_life(30)", "set_life(90)"));
        }

        #[test]
        fn literal_changed_on_disk_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("shot.rs");
            std::fs::write(&file, STATUS.replace("1.6f32", "1.9f32")).unwrap();
            let scale = setting(&file, STATUS, SettingField::Scale, SettingValue::Literal(1.6), "1.6f32");
            let error = plan_setting_write(&scale, 2.0, 0, &[]).unwrap_err().to_string();
            assert!(error.contains("changed"), "{error}");
        }

        #[test]
        fn computed_setting_refuses() {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("shot.rs");
            std::fs::write(&file, STATUS).unwrap();
            let computed = setting(
                &file,
                STATUS,
                SettingField::PowerMul,
                SettingValue::Computed("charge_boost + 1.0".into()),
                "30.0",
            );
            let error = plan_setting_write(&computed, 2.0, 0, &[]).unwrap_err().to_string();
            assert!(error.contains("worked out"), "{error}");
            assert_eq!(std::fs::read_to_string(&file).unwrap(), STATUS);
        }

        #[test]
        fn const_setting_becomes_a_const_edit() {
            let file = PathBuf::from("shot.rs");
            let value = SettingValue::Const { name: "FIREBALL_SCALE".into(), value: Some(1.4) };
            let scale = setting(&file, "FIREBALL_SCALE", SettingField::Scale, value, "FIREBALL_SCALE");
            assert_eq!(
                plan_setting_write(&scale, 2.5, 0, &[]).unwrap(),
                SettingWrite::Const(ConstEdit { name: "FIREBALL_SCALE".into(), value: "2.5".into() })
            );
            let unknown = Setting {
                value: SettingValue::Const { name: "FROM_ELSEWHERE".into(), value: None },
                ..scale
            };
            let error = plan_setting_write(&unknown, 2.5, 0, &[]).unwrap_err().to_string();
            assert!(error.contains("FROM_ELSEWHERE"), "{error}");

            // The definition keeps its own spelling when the edit is applied.
            let dir = tempfile::tempdir().unwrap();
            let defs = dir.path().join("consts.rs");
            let text = "const FIREBALL_SCALE: f32 = 1.4f32;\nconst FIREBALL_LIFE: i32 = 180;\n";
            std::fs::write(&defs, text).unwrap();
            let consts = crate::project_consts::ProjectConsts::scan(&[(defs.clone(), text.to_string())]);
            let edits = [
                ConstEdit { name: "FIREBALL_SCALE".into(), value: "2".into() },
                ConstEdit { name: "FIREBALL_LIFE".into(), value: "200.5".into() },
            ];
            let (_, notes, changed) = crate::project_consts::apply_edits(&consts, &edits);
            assert_eq!(changed, 1, "{notes:?}");
            assert!(notes.iter().any(|n| n.contains("FIREBALL_LIFE") && n.contains("whole-number")), "{notes:?}");
            assert_eq!(std::fs::read_to_string(&defs).unwrap(), text.replace("1.4f32", "2.0f32"));
        }

        fn prc_with(path: &Path, fields: &[(&str, prc::ParamKind)]) {
            let inner = prc::ParamStruct(fields.iter().map(|(n, v)| (hash40::hash40(n), v.clone())).collect());
            let root = prc::ParamStruct(vec![(hash40::hash40("param_cshot"), prc::ParamKind::Struct(inner))]);
            prc::save(path, &root).unwrap();
        }

        #[test]
        fn param_setting_targets_existing_layer() {
            let dir = tempfile::tempdir().unwrap();
            let (dump, romfs) = (dir.path().join("dump.prc"), dir.path().join("romfs.prcxml"));
            let dump_fields = [("speed", prc::ParamKind::Float(1.0)), ("life", prc::ParamKind::I32(60))];
            prc_with(&dump, &dump_fields);
            let dump_bytes = std::fs::read(&dump).unwrap();
            std::fs::write(
                &romfs,
                "<struct>\r\n  <struct hash=\"param_cshot\">\r\n    <float hash=\"speed\">2</float>\r\n  </struct>\r\n</struct>\r\n",
            )
            .unwrap();
            let file = PathBuf::from("shot.rs");
            let param = |field: &str| {
                setting(
                    &file,
                    "x",
                    SettingField::SpeedX,
                    SettingValue::Param { block: "param_cshot".into(), field: field.into() },
                    "x",
                )
            };
            let own = [romfs.clone()];
            // The moveset's own prcxml has `speed`, and so does the dump: the prcxml is chosen.
            assert_eq!(
                plan_setting_write(&param("speed"), 3.0, 0, &own).unwrap(),
                SettingWrite::Param { file: romfs.clone(), block: "param_cshot".into(), field: "speed".into(), variant: 0 }
            );
            // Only the dump has `life`: the dump is never chosen, and the error says what to do.
            let error = plan_setting_write(&param("life"), 70.0, 0, &own).unwrap_err().to_string();
            assert!(error.contains("`life` isn't in param_cshot in this moveset's own param file yet"), "{error}");
            assert!(error.contains(&romfs.display().to_string()), "{error}");
            // Not even when the moveset has no param file of its own.
            let error = plan_setting_write(&param("life"), 70.0, 0, &[]).unwrap_err().to_string();
            assert!(error.contains("no param file of its own"), "{error}");
            // An own file above the one that has the field could not be read: which file the
            // game reads is unknown, so nothing is chosen.
            let broken = dir.path().join("broken.prc");
            std::fs::write(&broken, b"not a param file").unwrap();
            let error = plan_setting_write(&param("speed"), 3.0, 0, &[romfs.clone(), broken]).unwrap_err().to_string();
            assert!(error.contains("could not read"), "{error}");

            // The planned write lands through the param writer, changing only that value.
            let SettingWrite::Param { file, block, field, variant } =
                plan_setting_write(&param("speed"), 3.0, 0, &own).unwrap()
            else {
                unreachable!()
            };
            crate::articles::params::write_param_field(&file, &block, &field, variant, 3.0, &HashMap::new()).unwrap();
            assert!(std::fs::read_to_string(&romfs).unwrap().contains("    <float hash=\"speed\">3</float>\r\n"));
            assert_eq!(std::fs::read(&dump).unwrap(), dump_bytes, "the dump is untouched");
        }

        /// R11b: a list block's variant 2 of 3 is checked and written, not variant 0.
        #[test]
        fn param_setting_targets_a_list_variant() {
            let dir = tempfile::tempdir().unwrap();
            let romfs = dir.path().join("vl.prcxml");
            let before = "<struct>\n<list hash=\"param_lativolt\">\n<struct index=\"0\"><float hash=\"speed\">1</float></struct>\n<struct index=\"1\"><float hash=\"speed\">2</float></struct>\n<struct index=\"2\"><float hash=\"speed\">3</float><int hash=\"life\">40</int></struct>\n</list>\n</struct>\n";
            std::fs::write(&romfs, before).unwrap();
            let file = PathBuf::from("shot.rs");
            let life = setting(
                &file,
                "x",
                SettingField::Life,
                SettingValue::Param { block: "param_lativolt".into(), field: "life".into() },
                "x",
            );
            let own = [romfs.clone()];
            let planned = plan_setting_write(&life, 55.0, 2, &own).unwrap();
            assert_eq!(
                planned,
                SettingWrite::Param { file: romfs.clone(), block: "param_lativolt".into(), field: "life".into(), variant: 2 }
            );
            let error = plan_setting_write(&life, 55.0, 1, &own).unwrap_err().to_string();
            assert!(error.contains("(list entry 1)"), "{error}");
            let SettingWrite::Param { file, block, field, variant } = planned else { unreachable!() };
            crate::articles::params::write_param_field(&file, &block, &field, variant, 55.0, &HashMap::new()).unwrap();
            assert_eq!(std::fs::read_to_string(&romfs).unwrap(), before.replace(">40<", ">55<"));
        }

        #[test]
        fn per_frame_vy_sub_is_gravity_and_file_local_consts_resolve() {
            use crate::articles::settings::{read_settings, StatusSite};
            use crate::project_consts::ProjectConsts;
            let dir = tempfile::tempdir().unwrap();
            let carrot_file = dir.path().join("carrot.rs");
            let other_file = dir.path().join("special_hi.rs");
            let carrot_src = r#"
const LAUNCH_VX: f32 = 0.5;
const LAUNCH_VY: f32 = 3.1;
const REFLECT_BOUNCE_VY: f32 = 1.8;
const ARC_GRAVITY: f32 = 0.12;
const LIFETIME_FRAMES: i32 = 250;

pub(super) unsafe extern "C" fn carrot_main(weapon: &mut L2CWeaponCommon) -> L2CValue {
    let owner_facing = PostureModule::lr(owner_boma);
    sv_kinetic_energy!(set_speed, weapon, WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL, LAUNCH_VX * owner_facing, LAUNCH_VY);
    weapon.set_life(LIFETIME_FRAMES);
    weapon.fast_shift_status(carrot_main_loop)
}

unsafe extern "C" fn carrot_main_loop(weapon: &mut L2CWeaponCommon) -> L2CValue {
    let current_vy = KineticModule::get_sum_speed_y(weapon.module_accessor, *KINETIC_ENERGY_RESERVE_ATTRIBUTE_MAIN);
    let facing = PostureModule::lr(weapon.module_accessor);
    let new_vy = if reflected_facing.is_some() { REFLECT_BOUNCE_VY } else { current_vy - ARC_GRAVITY };
    sv_kinetic_energy!(set_speed, weapon, WEAPON_KINETIC_ENERGY_RESERVE_ID_NORMAL, LAUNCH_VX * facing, new_vy);
    0.into()
}
"#;
            let other_src = "const LAUNCH_VY: f32 = 2.5;\n";
            std::fs::write(&carrot_file, carrot_src).unwrap();
            std::fs::write(&other_file, other_src).unwrap();
            let consts = ProjectConsts::scan(&[
                (carrot_file.clone(), carrot_src.to_string()),
                (other_file.clone(), other_src.to_string()),
            ]);
            assert!(consts.get("LAUNCH_VY").is_none(), "project-wide LAUNCH_VY should be ambiguous");

            let main_span = crate::acmd_src::function_item(carrot_src, "carrot_main").unwrap();
            let loop_span = crate::acmd_src::function_item(carrot_src, "carrot_main_loop").unwrap();
            let site = StatusSite {
                kind_expr: "WEAPON_PACMAN_BIGPACMAN_STATUS_KIND_CARROT".into(),
                phase: "Main".into(),
                function: "carrot_main".into(),
                file: carrot_file.clone(),
                span: main_span.clone(),
            };
            let settings = read_settings(
                &[(&site, carrot_src[main_span].to_string())],
                &consts,
                &|_caller, name| {
                    (name == "carrot_main_loop").then(|| {
                        (carrot_file.clone(), loop_span.clone(), carrot_src[loop_span.clone()].to_string())
                    })
                },
            );
            let speed_y = settings
                .iter()
                .find(|s| s.field == SettingField::SpeedY && matches!(s.value, SettingValue::Const { .. }))
                .expect("carrot LAUNCH_VY should resolve from carrot.rs");
            assert_eq!(speed_y.value, SettingValue::Const { name: "LAUNCH_VY".into(), value: Some(3.1) });
            let gravity = settings
                .iter()
                .find(|s| s.field == SettingField::Gravity)
                .expect("carrot_main_loop `current_vy - ARC_GRAVITY` should extract Gravity");
            assert_eq!(gravity.value, SettingValue::Const { name: "ARC_GRAVITY".into(), value: Some(0.12) });
            let life = settings
                .iter()
                .find(|s| s.field == SettingField::Life)
                .expect("carrot LIFETIME_FRAMES should resolve from carrot.rs");
            assert_eq!(life.value, SettingValue::Const { name: "LIFETIME_FRAMES".into(), value: Some(250.0) });
        }
    }
}
