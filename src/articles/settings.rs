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
/// calls (one level deep, through `helpers`, which maps a function name to its file, its byte
/// span in that file, and its source text).
pub fn read_settings(
    statuses: &[(&StatusSite, String)],
    consts: &ProjectConsts,
    helpers: &dyn Fn(&str) -> Option<(PathBuf, Range<usize>, String)>,
) -> Vec<Setting> {
    let mut out = Vec::new();
    for (site, body) in statuses {
        let status = Status { kind: &site.kind_expr, phase: &site.phase };
        let called = scan(body, &site.file, site.span.start, consts, status, &mut out);
        let mut followed: Vec<&str> = Vec::new();
        for name in &called {
            if name == &site.function || followed.contains(&name.as_str()) {
                continue;
            }
            followed.push(name);
            if let Some((file, span, text)) = helpers(name) {
                // One level only: what the helper calls is not followed.
                scan(&text, &file, span.start, consts, status, &mut out);
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

/// Records the settings `text` (one function, at byte `base` of `file`) writes, and returns the
/// names it references that may be project functions.
fn scan(
    text: &str,
    file: &Path,
    base: usize,
    consts: &ProjectConsts,
    status: Status,
    out: &mut Vec<Setting>,
) -> Vec<String> {
    let Ok(parsed) = syn::parse_file(text) else {
        return Vec::new();
    };
    let lines = LineIndex::new(text);
    let mut called = Vec::new();
    for item in &parsed.items {
        let syn::Item::Fn(function) = item else { continue };
        let mut lets = LetCollector { lines: &lines, lets: Vec::new() };
        syn::visit::Visit::visit_item_fn(&mut lets, function);
        let mut scanner = Scanner {
            ctx: Ctx { text, lines: &lines, lets: &lets.lets, consts },
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

/// `let NAME = INIT;` bindings of a function, with the byte offset each starts at, so a use reads
/// the binding in scope before it.
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
            let at = self.lines.offset(syn::spanned::Spanned::span(local).start());
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

    /// The `let` binding `name` in scope at byte `at`, with the offset it starts at.
    fn binding(&self, name: &str, at: usize) -> Option<(usize, &syn::Expr)> {
        self.lets
            .iter()
            .filter(|(n, start, _)| n == name && *start < at)
            .max_by_key(|(_, start, _)| *start)
            .map(|(_, start, init)| (*start, init))
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
            let value = self.consts.get(&name).and_then(|def| def.number());
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
        read_with(text, name, &|_| None)
    }

    fn read_with(
        text: &str,
        name: &str,
        helpers: &dyn Fn(&str) -> Option<(PathBuf, Range<usize>, String)>,
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
        let helpers = |name: &str| match name {
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
}
