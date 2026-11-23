//! Reads the `smashline::clone_weapon` registrations a moveset makes.
//!
//! A moveset spawns a cloned article (a projectile or fighter object it borrowed from another
//! fighter) by registering it with smashline at startup. Two signatures exist in the wild:
//!
//! ```text
//! old:  clone_weapon(src_owner, src_kind, new_owner, new_name, use_original_code) -> i32
//! new:  clone_weapon(new_owner, new_name, src_kind, use_original_code) -> CloneWeapon
//! ```
//!
//! The old one returns the generate id directly, which authors add to the last vanilla id
//! (`X += clone_weapon(..)`, `X = 6 + clone_weapon(..)`). The new one returns a struct whose
//! `generate_id` and `weapon_id` the author stores (`A.get_or_init(|| v.generate_id)`).
//!
//! This reader is pure and has no UI. A call it cannot model is reported as an
//! [`UnresolvedClone`] with the reason; it is never dropped.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::project_consts::ProjectConsts;

/// One resolved `clone_weapon` registration.
#[derive(Debug, Clone, PartialEq)]
pub struct CloneDecl {
    pub new_owner: String,
    pub new_name: String,
    /// The fighter the source article belongs to. Always known for the old signature; for the new
    /// one it is inferred from `src_kind` and may be `None`.
    pub src_owner: Option<String>,
    /// `WEAPON_KIND_MARIO_FIREBALL`: no `*`, no path.
    pub src_kind: String,
    pub use_original_code: bool,
    /// Names the generate id is stored in.
    pub id_bindings: Vec<String>,
    /// Names the clone's own weapon kind is stored in (new signature only).
    pub kind_bindings: Vec<String>,
    /// Why `id_bindings` is empty when the new signature was used: the generate id is stored
    /// somewhere the reader does not follow. `None` when bound, or for the old signature (whose
    /// id is legitimately discarded by a bare statement).
    pub id_note: Option<String>,
    pub file: PathBuf,
    pub line: usize,
}

/// A `clone_weapon` call that could not be modelled, kept with the source text and the reason.
#[derive(Debug, Clone, PartialEq)]
pub struct UnresolvedClone {
    pub file: PathBuf,
    pub line: usize,
    pub text: String,
    pub reason: String,
}

pub fn read_clones(
    sources: &[(PathBuf, String)],
    consts: &ProjectConsts,
    fighter_dirs: &[String],
) -> (Vec<CloneDecl>, Vec<UnresolvedClone>) {
    // `ProjectConsts` holds numbers only; the owner of a clone is usually a `&str` const.
    let mut named = NamedConsts::default();
    let parsed: Vec<(&PathBuf, &str, Option<syn::File>)> = sources
        .iter()
        .map(|(file, text)| (file, text.as_str(), syn::parse_file(text).ok()))
        .collect();
    // Every const and static by name, whatever its value, for the conventional id below.
    let mut defined = DefinedItems::default();
    for (_, _, ast) in &parsed {
        if let Some(ast) = ast {
            syn::visit::Visit::visit_file(&mut named, ast);
            syn::visit::Visit::visit_file(&mut defined, ast);
        }
    }
    let _ = consts; // numeric consts never name an owner, a kind or an article

    let mut out = Reader {
        file: PathBuf::new(),
        lines: Vec::new(),
        named: &named,
        fighter_dirs,
        env: Vec::new(),
        // Helpers from every file: a moveset's `util.rs` often holds the one its fighters call.
        helpers: parsed
            .iter()
            .filter_map(|(file, _, ast)| Some(clone_helpers(ast.as_ref()?, file)))
            .flatten()
            .collect(),
        inlining: Vec::new(),
        inlined: std::collections::HashSet::new(),
        decls: Vec::new(),
        unresolved: Vec::new(),
    };
    for (file, text, ast) in &parsed {
        out.file = (*file).clone();
        out.lines = text.lines().collect();
        match ast {
            Some(ast) => syn::visit::Visit::visit_file(&mut out, ast),
            None => out.text_scan(text),
        }
    }
    // A helper no call was followed into still makes clones: each is listed, never dropped.
    for helper in &out.helpers {
        if out.inlined.contains(&(helper.file.clone(), helper.line)) {
            continue;
        }
        let lines: Vec<&str> = parsed
            .iter()
            .find(|(file, _, _)| **file == helper.file)
            .map(|(_, text, _)| text.lines().collect())
            .unwrap_or_default();
        for line in clone_call_lines(&helper.body) {
            out.unresolved.push(UnresolvedClone {
                file: helper.file.clone(),
                line,
                text: lines
                    .get(line - 1)
                    .map(|l| l.trim().to_string())
                    .unwrap_or_default(),
                reason: format!(
                    "made inside helper `{}`, whose callers Visionary couldn't follow",
                    helper.name
                ),
            });
        }
    }
    let mut decls = out.decls;
    bind_conventional_ids(&mut decls, &defined.0);
    (decls, out.unresolved)
}

/// Every `const` and `static` the project defines, by name, whatever its value.
#[derive(Default)]
struct DefinedItems(std::collections::HashSet<String>);

impl<'ast> syn::visit::Visit<'ast> for DefinedItems {
    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        self.0.insert(item.ident.to_string());
    }
    fn visit_item_static(&mut self, item: &'ast syn::ItemStatic) {
        self.0.insert(item.ident.to_string());
    }
}

/// A clone whose call keeps no id is still bound when the project defines a const or static under the
/// conventional name, `FIGHTER_<OWNER>_GENERATE_ARTICLE_<NAME>`: that const is the name its
/// scripts spawn it by (Master Chief writes the number out, `= 0x9`, beside a bare call).
fn bind_conventional_ids(decls: &mut [CloneDecl], defined: &std::collections::HashSet<String>) {
    for decl in decls.iter_mut().filter(|d| d.id_bindings.is_empty()) {
        let conventional = format!(
            "FIGHTER_{}_GENERATE_ARTICLE_{}",
            decl.new_owner.to_uppercase(),
            decl.new_name.to_uppercase()
        );
        if defined.contains(&conventional) {
            decl.id_bindings.push(conventional);
            decl.id_note = None;
        }
    }
}

/// `const NAME: &str = "…";` and `const NAME: T = path::WEAPON_KIND_X;` across the project. A
/// name defined twice with different values is dropped, like [`ProjectConsts::scan`] does.
#[derive(Default)]
struct NamedConsts {
    strings: HashMap<String, Option<String>>,
    kinds: HashMap<String, Option<String>>,
}

impl NamedConsts {
    fn note(map: &mut HashMap<String, Option<String>>, name: String, value: String) {
        map.entry(name)
            .and_modify(|known| {
                if known.as_deref() != Some(value.as_str()) {
                    *known = None;
                }
            })
            .or_insert(Some(value));
    }
    fn string(&self, name: &str) -> Option<&str> {
        self.strings.get(name)?.as_deref()
    }
    fn kind(&self, name: &str) -> Option<&str> {
        self.kinds.get(name)?.as_deref()
    }
}

impl<'ast> syn::visit::Visit<'ast> for NamedConsts {
    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        let name = item.ident.to_string();
        match strip(&item.expr) {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) => {
                Self::note(&mut self.strings, name, s.value());
            }
            syn::Expr::Path(p) => {
                if let Some(last) = p.path.segments.last() {
                    Self::note(&mut self.kinds, name, last.ident.to_string());
                }
            }
            _ => {}
        }
        syn::visit::visit_item_const(self, item);
    }
}

/// Parens, groups and a leading `*` or `&` do not change which value an argument names.
fn strip(mut expr: &syn::Expr) -> &syn::Expr {
    loop {
        expr = match expr {
            syn::Expr::Paren(p) => &p.expr,
            syn::Expr::Group(g) => &g.expr,
            syn::Expr::Reference(r) => &r.expr,
            syn::Expr::Unary(u) if matches!(u.op, syn::UnOp::Deref(_)) => &u.expr,
            _ => return expr,
        };
    }
}

fn is_clone_call(call: &syn::ExprCall) -> bool {
    matches!(call.func.as_ref(), syn::Expr::Path(p)
        if p.path.segments.last().is_some_and(|s| s.ident == "clone_weapon"))
}

/// The `clone_weapon(..)` an expression is, directly or as one side of `N + clone_weapon(..)`.
fn find_clone_call(expr: &syn::Expr) -> Option<&syn::ExprCall> {
    match expr {
        syn::Expr::Call(c) if is_clone_call(c) => Some(c),
        syn::Expr::Paren(p) => find_clone_call(&p.expr),
        syn::Expr::Group(g) => find_clone_call(&g.expr),
        syn::Expr::Cast(c) => find_clone_call(&c.expr),
        syn::Expr::Unsafe(u) => match u.block.stmts.as_slice() {
            [syn::Stmt::Expr(e, None)] => find_clone_call(e),
            _ => None,
        },
        syn::Expr::Binary(b) if matches!(b.op, syn::BinOp::Add(_)) => {
            find_clone_call(&b.left).or_else(|| find_clone_call(&b.right))
        }
        _ => None,
    }
}

fn path_name(expr: &syn::Expr) -> Option<String> {
    match strip(expr) {
        syn::Expr::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

struct Reader<'a> {
    file: PathBuf,
    lines: Vec<&'a str>,
    named: &'a NamedConsts,
    fighter_dirs: &'a [String],
    /// Loop variables in scope, innermost last, each bound to one element of a literal table.
    /// While a helper is read for one of its calls, its parameters are bound to that call's
    /// arguments here too. A binding's value names only bindings before it (see
    /// [`Reader::lookup_env`]), so no chain of them can loop.
    env: Vec<(String, syn::Expr)>,
    /// The project's helper functions that wrap `clone_weapon` (see [`CloneHelper`]).
    helpers: Vec<CloneHelper>,
    /// The helpers being read for a call, innermost last (by file and line), with that call's
    /// line and text: a declaration made inside one is placed at the call, and a helper never
    /// inlines itself.
    inlining: Vec<((PathBuf, usize), usize, String)>,
    /// Helpers some call was followed into.
    inlined: std::collections::HashSet<(PathBuf, usize)>,
    decls: Vec<CloneDecl>,
    unresolved: Vec<UnresolvedClone>,
}

/// A function that calls `clone_weapon` with its own parameters, such as Sceptile's
/// `unsafe fn clone(name: &str, source: i32, generate_id: *mut i32, ..)`. Each call of it with
/// literal arguments is one clone, read as if the body were written at the call.
#[derive(Clone)]
struct CloneHelper {
    name: String,
    params: Vec<String>,
    body: syn::Block,
    file: PathBuf,
    /// The line of its `fn`, which tells it from other functions of the same name.
    line: usize,
}

/// The `clone_weapon` calls in `block`, by line.
fn clone_call_lines(block: &syn::Block) -> Vec<usize> {
    #[derive(Default)]
    struct Lines(Vec<usize>);
    impl<'ast> syn::visit::Visit<'ast> for Lines {
        fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
            use syn::spanned::Spanned;
            if is_clone_call(call) {
                self.0.push(call.span().start().line);
            }
            syn::visit::visit_expr_call(self, call);
        }
    }
    let mut lines = Lines::default();
    syn::visit::Visit::visit_block(&mut lines, block);
    lines.0
}

/// The helpers in `file`: functions, at any depth, with a `clone_weapon` call that reads one of
/// their parameters.
fn clone_helpers(file: &syn::File, path: &PathBuf) -> Vec<CloneHelper> {
    struct Finder<'p>(Vec<CloneHelper>, &'p PathBuf);
    impl<'ast> syn::visit::Visit<'ast> for Finder<'_> {
        fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
            let params: Vec<String> = item
                .sig
                .inputs
                .iter()
                .filter_map(|input| match input {
                    syn::FnArg::Typed(t) => match t.pat.as_ref() {
                        syn::Pat::Ident(p) => Some(p.ident.to_string()),
                        _ => None,
                    },
                    _ => None,
                })
                .collect();
            if !params.is_empty() && clone_reads_any(&item.block, &params) {
                use syn::spanned::Spanned;
                self.0.push(CloneHelper {
                    name: item.sig.ident.to_string(),
                    params,
                    body: (*item.block).clone(),
                    file: self.1.clone(),
                    line: item.sig.fn_token.span().start().line,
                });
            }
            syn::visit::visit_item_fn(self, item);
        }
    }
    let mut finder = Finder(Vec::new(), path);
    syn::visit::Visit::visit_file(&mut finder, file);
    finder.0
}

/// How deep a chain of bindings is followed: a backstop, since [`Reader::lookup_env`] only ever
/// looks backwards.
const MAX_BINDING_DEPTH: usize = 32;

/// Whether a `clone_weapon` call in `block` names one of `params` among its arguments.
fn clone_reads_any(block: &syn::Block, params: &[String]) -> bool {
    struct Probe<'p> {
        params: &'p [String],
        in_clone: bool,
        found: bool,
    }
    impl<'ast> syn::visit::Visit<'ast> for Probe<'_> {
        fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
            let was = self.in_clone;
            self.in_clone |= is_clone_call(call);
            syn::visit::visit_expr_call(self, call);
            self.in_clone = was;
        }
        fn visit_expr_path(&mut self, p: &'ast syn::ExprPath) {
            if self.in_clone && p.path.segments.len() == 1 {
                let name = p.path.segments[0].ident.to_string();
                self.found |= self.params.contains(&name);
            }
        }
    }
    let mut probe = Probe {
        params,
        in_clone: false,
        found: false,
    };
    syn::visit::Visit::visit_block(&mut probe, block);
    probe.found
}

/// The static a pointer argument points at: `std::ptr::addr_of_mut!(X)`, `addr_of!(X)`,
/// `&mut X` or `&raw mut X`.
fn pointee(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Paren(p) => pointee(&p.expr),
        syn::Expr::Group(g) => pointee(&g.expr),
        syn::Expr::Reference(r) => target_name(&r.expr),
        syn::Expr::RawAddr(r) => target_name(&r.expr),
        syn::Expr::Macro(m) => {
            let last = m.mac.path.segments.last()?.ident.to_string();
            if last != "addr_of_mut" && last != "addr_of" {
                return None;
            }
            target_name(&m.mac.parse_body::<syn::Expr>().ok()?)
        }
        _ => None,
    }
}

const UNBOUND_ID: &str = "the generate id is stored somewhere this reader does not follow (a destructured `let`, a pointer or an unnamed target), so no name is known for it";

/// A name an id or kind is stored into: `A`, `path::A`, or the field in `self.A`. A pointer
/// (`*slot`) names nothing.
fn target_name(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Paren(p) => target_name(&p.expr),
        syn::Expr::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        syn::Expr::Field(f) => match &f.member {
            syn::Member::Named(m) => Some(m.to_string()),
            _ => None,
        },
        _ => None,
    }
}

/// `clone_weapon(..).generate_id` / `.weapon_id` written inline: the call and which field.
fn inline_field(expr: &syn::Expr) -> Option<(&syn::ExprCall, &'static str)> {
    match expr {
        syn::Expr::Field(f) => {
            let kind = match &f.member {
                syn::Member::Named(m) if m == "generate_id" => "id",
                syn::Member::Named(m) if m == "weapon_id" => "kind",
                _ => return None,
            };
            find_clone_call(&f.base).map(|c| (c, kind))
        }
        syn::Expr::Paren(p) => inline_field(&p.expr),
        syn::Expr::Group(g) => inline_field(&g.expr),
        syn::Expr::Closure(c) => inline_field(&c.body),
        syn::Expr::Block(b) => match b.block.stmts.as_slice() {
            [syn::Stmt::Expr(e, None)] => inline_field(e),
            _ => None,
        },
        _ => None,
    }
}

type Args<'a> = Vec<&'a syn::Expr>;

enum HelperLookup {
    None,
    One(CloneHelper),
    Ambiguous(Vec<CloneHelper>),
}

/// Module-name components of a Rust source file: directory segments plus the file stem, omitting
/// `mod`, `lib`, and `main` (so `src/util/mod.rs` yields `["src", "util"]` and `other.rs` yields
/// `["other"]`).
fn file_module_trail(file: &std::path::Path) -> Vec<String> {
    let mut parts: Vec<String> = file
        .parent()
        .into_iter()
        .flat_map(|p| p.components())
        .filter_map(|c| match c {
            std::path::Component::Normal(os) => os.to_str().map(str::to_owned),
            _ => None,
        })
        .collect();
    if let Some(stem) = file.file_stem().and_then(|s| s.to_str()) {
        if !matches!(stem, "mod" | "lib" | "main") {
            parts.push(stem.to_owned());
        }
    }
    parts
}

impl Reader<'_> {
    fn line_of(span: proc_macro2::Span) -> usize {
        span.start().line
    }

    /// The helper a call runs: `clone(..)`, `self::clone(..)`, `util::clone_article(..)`, by its
    /// module path (when qualified), last segment, and argument count. A bare call prefers one in
    /// the calling file; a module-qualified call (`other::clone(..)`) matches only helpers whose
    /// file path ends with that module trail and never falls back to a local helper. A path
    /// through a type (`Arc::clone(..)`) is never a helper.
    fn helper_for(&self, call: &syn::ExprCall) -> HelperLookup {
        let syn::Expr::Path(p) = call.func.as_ref() else {
            return HelperLookup::None;
        };
        let segments: Vec<String> = p
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        let Some((name, modules)) = segments.split_last() else {
            return HelperLookup::None;
        };
        if modules
            .iter()
            .any(|m| m.starts_with(|c: char| c.is_ascii_uppercase()))
        {
            return HelperLookup::None;
        }
        let fits: Vec<&CloneHelper> = self
            .helpers
            .iter()
            .filter(|h| h.name == *name && h.params.len() == call.args.len())
            .filter(|h| {
                !self
                    .inlining
                    .iter()
                    .any(|((file, line), _, _)| *file == h.file && *line == h.line)
            })
            .collect();
        if fits.is_empty() {
            return HelperLookup::None;
        }
        let pick = |list: Vec<&CloneHelper>| match list.as_slice() {
            [] => HelperLookup::None,
            [one] => HelperLookup::One((*one).clone()),
            _ => HelperLookup::Ambiguous(list.into_iter().cloned().collect()),
        };
        let named_modules: Vec<&str> = modules
            .iter()
            .map(String::as_str)
            .skip_while(|m| matches!(*m, "crate" | "super" | "self"))
            .collect();
        if !named_modules.is_empty() {
            let matched: Vec<&CloneHelper> = fits
                .into_iter()
                .filter(|h| {
                    let trail = file_module_trail(&h.file);
                    trail.len() >= named_modules.len()
                        && trail[trail.len() - named_modules.len()..]
                            .iter()
                            .map(String::as_str)
                            .eq(named_modules.iter().copied())
                })
                .collect();
            return pick(matched);
        }
        if modules.iter().any(|m| m == "super") {
            let external: Vec<&CloneHelper> =
                fits.into_iter().filter(|h| h.file != self.file).collect();
            return pick(external);
        }
        let local: Vec<&CloneHelper> = fits
            .iter()
            .copied()
            .filter(|h| h.file == self.file)
            .collect();
        if modules.iter().any(|m| m == "self") {
            return pick(local);
        }
        if !local.is_empty() {
            return pick(local);
        }
        pick(fits)
    }

    /// The source lines a call covers, trimmed, for the report.
    fn text_of(&self, span: proc_macro2::Span) -> String {
        let (first, last) = (span.start().line, span.end().line.max(span.start().line));
        (first..=last)
            .filter_map(|n| self.lines.get(n - 1))
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn report(&mut self, call: &syn::ExprCall, reason: impl Into<String>) {
        use syn::spanned::Spanned;
        let span = call.span();
        let reason = reason.into();
        // Inside a helper: at the call that was followed, with the helper named.
        let (line, text, reason) = match self.inlining.first() {
            Some((_, line, text)) => {
                let helper = self.inlining.last().and_then(|((file, at), _, _)| {
                    self.helpers
                        .iter()
                        .find(|h| h.file == *file && h.line == *at)
                        .map(|h| h.name.clone())
                });
                (
                    *line,
                    text.clone(),
                    format!(
                        "{reason} (made inside helper `{}`)",
                        helper.unwrap_or_default()
                    ),
                )
            }
            None => (Self::line_of(span), self.text_of(span), reason),
        };
        let unresolved = UnresolvedClone {
            file: self.file.clone(),
            line,
            text,
            reason,
        };
        self.unresolved.push(unresolved);
    }

    /// An argument with a loop variable or an outer helper's parameter replaced by its value.
    fn resolved_arg(&self, arg: &syn::Expr) -> syn::Expr {
        let bare = matches!(strip(arg), syn::Expr::Path(p) if p.path.segments.len() == 1);
        match path_name(arg).and_then(|name| self.lookup_env(&name, self.env.len())) {
            Some((_, bound)) if bare => bound.clone(),
            _ => arg.clone(),
        }
    }

    /// The innermost binding of `name` among the first `scope` bindings, with its index. A
    /// binding's value is looked up among the bindings before it, which is the scope it was
    /// written in, so `name → name` or `a → b → a` can never be followed round.
    fn lookup_env(&self, name: &str, scope: usize) -> Option<(usize, &syn::Expr)> {
        self.env[..scope.min(self.env.len())]
            .iter()
            .enumerate()
            .rev()
            .find(|(_, (n, _))| n == name)
            .map(|(at, (_, e))| (at, e))
    }

    /// A string argument: a literal, a loop variable bound to one, or a `const &str`.
    fn string_arg(&self, expr: &syn::Expr, what: &str) -> Result<String, String> {
        self.string_arg_in(expr, what, self.env.len(), 0)
    }

    fn string_arg_in(
        &self,
        expr: &syn::Expr,
        what: &str,
        scope: usize,
        depth: usize,
    ) -> Result<String, String> {
        if depth > MAX_BINDING_DEPTH {
            return Err(format!(
                "the {what} goes through more bindings than Visionary follows"
            ));
        }
        match strip(expr) {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) => Ok(s.value()),
            syn::Expr::Path(p) if p.path.segments.len() == 1 => {
                let name = p.path.segments[0].ident.to_string();
                if let Some((at, bound)) = self.lookup_env(&name, scope) {
                    return self.string_arg_in(bound, what, at, depth + 1);
                }
                self.named.string(&name).map(str::to_string).ok_or_else(|| {
                    format!("the {what} is `{name}`, which is not a string literal or a `const &str` in the project")
                })
            }
            _ => Err(format!(
                "the {what} is computed, not a string literal or a `const &str`"
            )),
        }
    }

    /// A weapon kind argument, reduced to its bare `WEAPON_KIND_…` name.
    fn kind_arg(&self, expr: &syn::Expr) -> Result<String, String> {
        self.kind_arg_in(expr, self.env.len(), 0)
    }

    fn kind_arg_in(&self, expr: &syn::Expr, scope: usize, depth: usize) -> Result<String, String> {
        if depth > MAX_BINDING_DEPTH {
            return Err(
                "the source weapon kind goes through more bindings than Visionary follows".into(),
            );
        }
        let Some(name) = path_name(expr) else {
            return Err(
                "the source weapon kind is computed, not a `WEAPON_KIND_…` constant".into(),
            );
        };
        if let Some((at, bound)) = self.lookup_env(&name, scope) {
            return self.kind_arg_in(bound, at, depth + 1);
        }
        if name.starts_with("WEAPON_KIND_") {
            return Ok(name);
        }
        match self.named.kind(&name) {
            Some(kind) => Ok(kind.to_string()),
            None => Err(format!("the source weapon kind `{name}` is not a `WEAPON_KIND_…` constant or a const naming one")),
        }
    }

    fn bool_arg(expr: &syn::Expr) -> Result<bool, String> {
        match strip(expr) {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Bool(b),
                ..
            }) => Ok(b.value),
            _ => Err("use_original_code is not a literal `true` or `false`".into()),
        }
    }

    /// The new API names no source owner; the kind does (`WEAPON_KIND_<OWNER>_<NAME>`).
    fn owner_from_kind(&self, kind: &str) -> Option<String> {
        let rest = kind.strip_prefix("WEAPON_KIND_")?;
        self.fighter_dirs
            .iter()
            .filter(|dir| {
                let prefix = format!("{}_", dir.to_uppercase());
                !dir.is_empty() && rest.starts_with(&prefix)
            })
            .max_by_key(|dir| dir.len())
            .cloned()
    }

    /// Resolves one call into a declaration, or reports why it cannot.
    fn resolve(&mut self, call: &syn::ExprCall, discarded: bool) -> Option<usize> {
        use syn::spanned::Spanned;
        let args: Args = call.args.iter().collect();
        let built = match args.len() {
            5 => self.resolve_old(&args, discarded),
            4 => self.resolve_new(&args),
            n => Err(format!(
                "clone_weapon takes 4 or 5 arguments, this call has {n}"
            )),
        };
        match built {
            Ok(mut decl) => {
                decl.file = self.file.clone();
                decl.line = self
                    .inlining
                    .first()
                    .map_or_else(|| Self::line_of(call.span()), |(_, line, _)| *line);
                self.decls.push(decl);
                Some(self.decls.len() - 1)
            }
            Err(reason) => {
                self.report(call, reason);
                None
            }
        }
    }

    fn resolve_old(&self, a: &Args, discarded: bool) -> Result<CloneDecl, String> {
        Ok(CloneDecl {
            src_owner: Some(self.string_arg(a[0], "source owner")?),
            src_kind: self.kind_arg(a[1])?,
            new_owner: self.string_arg(a[2], "new owner")?,
            new_name: self.string_arg(a[3], "new article name")?,
            use_original_code: Self::bool_arg(a[4])?,
            id_bindings: Vec::new(),
            kind_bindings: Vec::new(),
            id_note: (!discarded).then(|| UNBOUND_ID.to_string()),
            file: PathBuf::new(),
            line: 0,
        })
    }

    fn resolve_new(&self, a: &Args) -> Result<CloneDecl, String> {
        let src_kind = self.kind_arg(a[2])?;
        Ok(CloneDecl {
            new_owner: self.string_arg(a[0], "new owner")?,
            new_name: self.string_arg(a[1], "new article name")?,
            src_owner: self.owner_from_kind(&src_kind),
            src_kind,
            use_original_code: Self::bool_arg(a[3])?,
            id_bindings: Vec::new(),
            kind_bindings: Vec::new(),
            id_note: Some(UNBOUND_ID.into()),
            file: PathBuf::new(),
            line: 0,
        })
    }

    /// A file that does not parse still gets every call site reported.
    fn text_scan(&mut self, text: &str) {
        for (index, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if !code.contains("clone_weapon(") {
                continue;
            }
            self.unresolved.push(UnresolvedClone {
                file: self.file.clone(),
                line: index + 1,
                text: line.trim().to_string(),
                reason: "the file could not be parsed".into(),
            });
        }
    }

    /// Declarations made by one statement `X = N + clone_weapon(..)` / `X += clone_weapon(..)`.
    fn assignment(&mut self, target: &syn::Expr, value: &syn::Expr) -> bool {
        if self.inline_store(target, value) {
            return true;
        }
        let Some(call) = find_clone_call(value) else {
            return false;
        };
        let id = self.resolve(call, false);
        if let (Some(index), Some(name)) = (id, target_name(target)) {
            self.decls[index].id_bindings.push(name);
            self.decls[index].id_note = None;
        }
        true
    }

    /// `A.get_or_init(|| clone_weapon(..).generate_id)` / `A = clone_weapon(..).weapon_id`.
    fn inline_store(&mut self, target: &syn::Expr, value: &syn::Expr) -> bool {
        let Some((call, field)) = inline_field(value) else {
            return false;
        };
        if let Some(index) = self.resolve(call, false) {
            if let Some(name) = target_name(target) {
                if field == "id" {
                    self.decls[index].id_bindings.push(name);
                    self.decls[index].id_note = None;
                } else {
                    self.decls[index].kind_bindings.push(name);
                }
            }
        }
        true
    }

    /// The decl indices a statement list's `let v = clone_weapon(..)` produced, with their binding
    /// names taken from the statements that follow it in the same block.
    fn let_binding(&mut self, local: &syn::Local, rest: &[syn::Stmt]) -> bool {
        let Some(init) = &local.init else {
            return false;
        };
        let Some(call) = find_clone_call(&init.expr) else {
            return false;
        };
        let var = match &local.pat {
            syn::Pat::Ident(p) => Some(p.ident.to_string()),
            _ => None,
        };
        let Some(index) = self.resolve(call, false) else {
            return true;
        };
        if let Some(var) = var {
            // The old signature returns the id itself, so the variable is the id.
            let direct = call.args.len() == 5;
            let pointers = self
                .env
                .iter()
                .filter_map(|(name, arg)| Some((name.clone(), pointee(arg)?)))
                .collect();
            let mut finder = BindingFinder {
                var,
                direct,
                aliases: Vec::new(),
                pointers,
                ids: Vec::new(),
                kinds: Vec::new(),
            };
            for stmt in rest {
                syn::visit::Visit::visit_stmt(&mut finder, stmt);
            }
            if !finder.ids.is_empty() {
                self.decls[index].id_note = None;
            }
            self.decls[index].id_bindings = finder.ids;
            self.decls[index].kind_bindings = finder.kinds;
        }
        true
    }
}

impl<'ast> syn::visit::Visit<'ast> for Reader<'_> {
    fn visit_block(&mut self, block: &'ast syn::Block) {
        for (i, stmt) in block.stmts.iter().enumerate() {
            if let syn::Stmt::Local(local) = stmt {
                if self.let_binding(local, &block.stmts[i + 1..]) {
                    continue;
                }
            }
            // `clone_weapon(..);` on its own: the id is discarded on purpose.
            if let syn::Stmt::Expr(syn::Expr::Call(call), _) = stmt {
                if is_clone_call(call) {
                    self.resolve(call, true);
                    continue;
                }
            }
            self.visit_stmt(stmt);
        }
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        if call.method == "get_or_init" || call.method == "set" {
            if let Some(arg) = call.args.first() {
                if self.inline_store(&call.receiver, arg) {
                    return;
                }
            }
        }
        syn::visit::visit_expr_method_call(self, call);
    }

    fn visit_expr_assign(&mut self, node: &'ast syn::ExprAssign) {
        if !self.assignment(&node.left, &node.right) {
            syn::visit::visit_expr_assign(self, node);
        }
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        if matches!(node.op, syn::BinOp::AddAssign(_)) && self.assignment(&node.left, &node.right) {
            return;
        }
        syn::visit::visit_expr_binary(self, node);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        use syn::spanned::Spanned;
        if is_clone_call(call) {
            self.resolve(call, false);
            return;
        }
        match self.helper_for(call) {
            HelperLookup::One(helper) => {
                let mark = self.env.len();
                // Arguments are read in the caller's scope, before the parameters shadow anything. A
                // parameter whose argument is an unknown name of its own (`clone(name)` inside
                // `fn reg(name: &str)`) is left unbound: it is unknown, not itself.
                let bound: Vec<(String, syn::Expr)> = helper
                    .params
                    .iter()
                    .zip(&call.args)
                    .map(|(param, arg)| (param.clone(), self.resolved_arg(arg)))
                    .filter(|(param, value)| path_name(value).as_deref() != Some(param.as_str()))
                    .collect();
                self.env.extend(bound);
                let key = (helper.file.clone(), helper.line);
                self.inlined.insert(key.clone());
                let (line, text) = match self.inlining.first() {
                    Some((_, line, text)) => (*line, text.clone()),
                    None => (Self::line_of(call.span()), self.text_of(call.span())),
                };
                self.inlining.push((key, line, text));
                self.visit_block(&helper.body);
                self.inlining.pop();
                self.env.truncate(mark);
                return;
            }
            HelperLookup::Ambiguous(candidates) => {
                let mut where_list: Vec<String> = candidates
                    .iter()
                    .map(|h| format!("{}:{}", h.file.display(), h.line))
                    .collect();
                where_list.sort();
                let name = candidates
                    .first()
                    .map(|h| h.name.as_str())
                    .unwrap_or("helper");
                self.report(
                    call,
                    format!(
                        "ambiguous call to clone helper `{name}`; matches {}",
                        where_list.join(", ")
                    ),
                );
                return;
            }
            HelperLookup::None => {}
        }
        syn::visit::visit_expr_call(self, call);
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        use syn::spanned::Spanned;
        // A helper's own body is read at each of its calls, where its arguments are known. Only
        // that function: another of the same name is read as usual.
        let line = item.sig.fn_token.span().start().line;
        if self
            .helpers
            .iter()
            .any(|h| h.file == self.file && h.line == line)
        {
            return;
        }
        syn::visit::visit_item_fn(self, item);
    }

    fn visit_expr_for_loop(&mut self, node: &'ast syn::ExprForLoop) {
        let elements: Option<Vec<&syn::Expr>> = match strip(&node.expr) {
            syn::Expr::Array(a) => Some(a.elems.iter().collect()),
            _ => None,
        };
        let mut names = Vec::new();
        collect_pat_names(&node.pat, &mut names);
        let unrollable = elements.as_ref().is_some_and(|els| {
            els.iter().all(|e| match (&names, strip(e)) {
                (n, syn::Expr::Tuple(t)) if matches!(*node.pat, syn::Pat::Tuple(_)) => {
                    t.elems.len() == n.len()
                }
                (n, _) => n.len() == 1 && matches!(*node.pat, syn::Pat::Ident(_)),
            })
        });
        if !unrollable {
            // Not a literal table: any clone_weapon inside reports its own unresolved argument.
            syn::visit::visit_expr_for_loop(self, node);
            return;
        }
        for element in elements.unwrap() {
            let mark = self.env.len();
            match strip(element) {
                syn::Expr::Tuple(t) if matches!(*node.pat, syn::Pat::Tuple(_)) => {
                    for (name, value) in names.iter().zip(&t.elems) {
                        self.env.push((name.clone(), value.clone()));
                    }
                }
                other => self.env.push((names[0].clone(), other.clone())),
            }
            self.visit_block(&node.body);
            self.env.truncate(mark);
        }
    }
}

fn collect_pat_names(pat: &syn::Pat, out: &mut Vec<String>) {
    match pat {
        syn::Pat::Ident(p) => out.push(p.ident.to_string()),
        syn::Pat::Tuple(t) => t.elems.iter().for_each(|p| collect_pat_names(p, out)),
        syn::Pat::Wild(_) => out.push("_".into()),
        _ => out.push("?".into()),
    }
}

/// Finds where a `let v = clone_weapon(..)` result is stored: `A.get_or_init(|| v.generate_id)`,
/// `A.set(v.generate_id)` or `A = v.generate_id`.
struct BindingFinder {
    var: String,
    /// Old signature: the variable itself is the generate id.
    direct: bool,
    /// Names bound to a field read (`let id = v.generate_id;`), with which field.
    aliases: Vec<(String, &'static str)>,
    /// A helper's pointer parameters and the static each call points them at, so
    /// `*generate_id = v.generate_id` stores into that static.
    pointers: Vec<(String, String)>,
    ids: Vec<String>,
    kinds: Vec<String>,
}

impl BindingFinder {
    /// Which field of the clone an expression reads, if it reads one.
    fn field_of(&self, expr: &syn::Expr) -> Option<&'static str> {
        struct Probe<'v> {
            var: &'v str,
            direct: bool,
            aliases: &'v [(String, &'static str)],
            found: Option<&'static str>,
        }
        impl<'ast> syn::visit::Visit<'ast> for Probe<'_> {
            fn visit_expr_path(&mut self, p: &'ast syn::ExprPath) {
                if p.path.segments.len() == 1 {
                    let name = p.path.segments[0].ident.to_string();
                    if self.direct && name == self.var {
                        self.found = Some("id");
                    }
                    if let Some((_, field)) = self.aliases.iter().find(|(n, _)| *n == name) {
                        self.found = Some(field);
                    }
                }
            }

            fn visit_expr_field(&mut self, f: &'ast syn::ExprField) {
                if path_name(&f.base).as_deref() == Some(self.var) {
                    if let syn::Member::Named(m) = &f.member {
                        if m == "generate_id" {
                            self.found = Some("id");
                        } else if m == "weapon_id" {
                            self.found = Some("kind");
                        }
                    }
                }
                syn::visit::visit_expr_field(self, f);
            }
        }
        let mut probe = Probe {
            var: &self.var,
            direct: self.direct,
            aliases: &self.aliases,
            found: None,
        };
        syn::visit::Visit::visit_expr(&mut probe, expr);
        probe.found
    }

    fn store(&mut self, target: &syn::Expr, value: &syn::Expr) {
        let through_pointer = match target {
            syn::Expr::Unary(u) if matches!(u.op, syn::UnOp::Deref(_)) => path_name(&u.expr)
                .and_then(|param| {
                    self.pointers
                        .iter()
                        .find(|(p, _)| *p == param)
                        .map(|(_, to)| to.clone())
                }),
            _ => None,
        };
        let (Some(name), Some(field)) = (
            through_pointer.or_else(|| target_name(target)),
            self.field_of(value),
        ) else {
            return;
        };
        let list = if field == "id" {
            &mut self.ids
        } else {
            &mut self.kinds
        };
        if !list.contains(&name) {
            list.push(name);
        }
    }
}

impl<'ast> syn::visit::Visit<'ast> for BindingFinder {
    fn visit_local(&mut self, local: &'ast syn::Local) {
        if let (syn::Pat::Ident(p), Some(init)) = (&local.pat, &local.init) {
            if let Some(field) = self.field_of(&init.expr) {
                self.aliases.push((p.ident.to_string(), field));
            }
        }
        syn::visit::visit_local(self, local);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        if call.method == "get_or_init" || call.method == "set" {
            if let Some(arg) = call.args.first() {
                self.store(&call.receiver, arg);
            }
        }
        syn::visit::visit_expr_method_call(self, call);
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        if matches!(node.op, syn::BinOp::AddAssign(_)) {
            self.store(&node.left, &node.right);
        }
        syn::visit::visit_expr_binary(self, node);
    }

    fn visit_expr_assign(&mut self, node: &'ast syn::ExprAssign) {
        self.store(&node.left, &node.right);
        syn::visit::visit_expr_assign(self, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str) -> (Vec<CloneDecl>, Vec<UnresolvedClone>) {
        run_many(&[("lib.rs", src)])
    }

    fn run_many(files: &[(&str, &str)]) -> (Vec<CloneDecl>, Vec<UnresolvedClone>) {
        run_dirs(
            files,
            &[
                "mario",
                "miiswordsman",
                "link",
                "kingteresa",
                "mii",
                "buddy",
            ],
        )
    }

    fn run_dirs(files: &[(&str, &str)], dirs: &[&str]) -> (Vec<CloneDecl>, Vec<UnresolvedClone>) {
        let sources: Vec<(PathBuf, String)> = files
            .iter()
            .map(|(p, t)| (PathBuf::from(p), t.to_string()))
            .collect();
        let consts = ProjectConsts::scan(&sources);
        let dirs: Vec<String> = dirs.iter().map(|s| s.to_string()).collect();
        read_clones(&sources, &consts, &dirs)
    }

    /// Latios-Moveset · src/lib.rs:559 (old API, `+=`).
    #[test]
    fn old_api_plus_equals() {
        let (d, u) = run(r#"
pub fn install() {
    unsafe {
        FIGHTER_SAMUS_GENERATE_ARTICLE_LATIVOLT += smashline::clone_weapon("mario", *WEAPON_KIND_MARIO_FIREBALL, "samus", "lativolt", false);
    }
}
"#);
        assert!(u.is_empty(), "{u:?}");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].new_owner, "samus");
        assert_eq!(d[0].new_name, "lativolt");
        assert_eq!(d[0].src_owner.as_deref(), Some("mario"));
        assert_eq!(d[0].src_kind, "WEAPON_KIND_MARIO_FIREBALL");
        assert!(!d[0].use_original_code);
        assert_eq!(
            d[0].id_bindings,
            ["FIGHTER_SAMUS_GENERATE_ARTICLE_LATIVOLT"]
        );
        assert!(d[0].kind_bindings.is_empty());
        assert_eq!(d[0].line, 4);
    }

    /// Empoleon-Moveset · .temp/src_alt/emperte_bubble.rs:83 (old API, `= N + clone_weapon`).
    #[test]
    fn old_api_offset() {
        let (d, u) = run(r#"
fn reg() {
    unsafe {
        ARTICLE_ID = 6 + smashline::clone_weapon("miigunner", *WEAPON_KIND_MIIGUNNER_ATTACKAIRF_BULLET,
            "falco", "x", false);
    }
}
"#);
        assert!(u.is_empty(), "{u:?}");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].id_bindings, ["ARTICLE_ID"]);
        assert_eq!(d[0].src_owner.as_deref(), Some("miigunner"));
        assert_eq!(d[0].src_kind, "WEAPON_KIND_MIIGUNNER_ATTACKAIRF_BULLET");
    }

    /// King-Boo-Moveset · src/kingteresa/mod.rs:23-25 (new API, OnceLock).
    #[test]
    fn new_api_once_lock() {
        let (d, u) = run(r#"
pub fn smashline_install() {
    let fireball = smashline::clone_weapon("dedede", "fireball", *WEAPON_KIND_MIISWORDSMAN_WAVE, false);
    FIGHTER_KINGTERESA_GENERATE_ARTICLE_FIREBALL.get_or_init(|| fireball.generate_id);
    WEAPON_KIND_KINGTERESA_FIREBALL.get_or_init(|| fireball.weapon_id);

    let portrait = smashline::clone_weapon("dedede", "portrait", *WEAPON_KIND_DOLLY_CAP, false);
    FIGHTER_KINGTERESA_GENERATE_ARTICLE_PORTRAIT.set(portrait.generate_id).ok();
}
"#);
        assert!(u.is_empty(), "{u:?}");
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].new_owner, "dedede");
        assert_eq!(d[0].new_name, "fireball");
        assert_eq!(d[0].src_owner.as_deref(), Some("miiswordsman"));
        assert_eq!(d[0].src_kind, "WEAPON_KIND_MIISWORDSMAN_WAVE");
        assert_eq!(
            d[0].id_bindings,
            ["FIGHTER_KINGTERESA_GENERATE_ARTICLE_FIREBALL"]
        );
        assert_eq!(d[0].kind_bindings, ["WEAPON_KIND_KINGTERESA_FIREBALL"]);
        // Bindings never leak to the next clone; `.set(..)` counts like get_or_init.
        assert_eq!(
            d[1].id_bindings,
            ["FIGHTER_KINGTERESA_GENERATE_ARTICLE_PORTRAIT"]
        );
        assert!(d[1].kind_bindings.is_empty());
        // No fighter named dolly in the list, so the owner is unknown, not guessed.
        assert_eq!(d[1].src_owner, None);
    }

    /// Longest matching fighter wins: both `PTRAINER_` and `PTRAINER_LOW_` prefix the kind
    /// (synthetic dir names; a first-match or shortest-match would pick `ptrainer`).
    #[test]
    fn new_api_source_owner_prefers_longest() {
        let (d, _) = run_dirs(
            &[(
                "lib.rs",
                r#"fn f() { let a = clone_weapon("x", "y", *WEAPON_KIND_PTRAINER_LOW_BALL, false); }"#,
            )],
            &["ptrainer", "ptrainer_low", "mario"],
        );
        assert_eq!(d[0].src_owner.as_deref(), Some("ptrainer_low"));
    }

    /// Two-hop: `let id = v.generate_id; A.set(id)`.
    #[test]
    fn new_api_two_hop_binding() {
        let (d, u) = run(r#"
fn f() {
    let v = smashline::clone_weapon("dedede", "a", *WEAPON_KIND_LINK_SWORD_BEAM, false);
    let id = v.generate_id;
    let kind = v.weapon_id;
    ID.set(id).ok();
    self.KIND = kind;
}
"#);
        assert!(u.is_empty(), "{u:?}");
        assert_eq!(d[0].id_bindings, ["ID"]);
        assert_eq!(d[0].kind_bindings, ["KIND"]);
        assert_eq!(d[0].id_note, None);
    }

    /// Inline: `X.get_or_init(|| clone_weapon(..).generate_id)`.
    #[test]
    fn new_api_inline_get_or_init() {
        let (d, u) = run(r#"
fn f() {
    X.get_or_init(|| smashline::clone_weapon("dedede", "a", *WEAPON_KIND_LINK_SWORD_BEAM, false).generate_id);
}
"#);
        assert!(u.is_empty(), "{u:?}");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].id_bindings, ["X"]);
        assert_eq!(d[0].id_note, None);
    }

    /// Stored through a pointer, as Sceptile's helper does (`*generate_id = clone.generate_id`),
    /// or destructured: the decl is kept and says why it has no id name.
    #[test]
    fn new_api_stored_elsewhere_gives_reason() {
        let (d, u) = run(r#"
unsafe fn f(slot: *mut i32) {
    let clone = smashline::clone_weapon("mario", "a", *WEAPON_KIND_LINK_SWORD_BEAM, false);
    *slot = clone.generate_id;
    let b = smashline::clone_weapon("mario", "b", *WEAPON_KIND_LINK_SWORD_BEAM, false);
    let smashline::CloneWeapon { generate_id, .. } = b;
    let (a, b) = smashline::clone_weapon("mario", "c", *WEAPON_KIND_LINK_SWORD_BEAM, false);
}
"#);
        assert!(u.is_empty(), "{u:?}");
        assert_eq!(d.len(), 3);
        for decl in &d {
            assert!(decl.id_bindings.is_empty());
            let note = decl.id_note.as_deref().expect("a reason");
            assert!(note.contains("does not follow"), "{note}");
        }
    }

    /// Old API result stored in a variable, then in a name (two-hop), with `=` and `+=`.
    #[test]
    fn old_api_two_hop_binds() {
        let (d, u) = run(r#"
fn f() {
    let id = smashline::clone_weapon("link", *WEAPON_KIND_LINK_SWORD_BEAM, "samus", "a", false);
    A.set(id).ok();
    let id2 = smashline::clone_weapon("link", *WEAPON_KIND_LINK_SWORD_BEAM, "samus", "b", false);
    unsafe { B += id2; }
}
"#);
        assert!(u.is_empty(), "{u:?}");
        assert_eq!(d[0].id_bindings, ["A"]);
        assert_eq!(d[0].id_note, None);
        assert_eq!(d[1].id_bindings, ["B"]);
        assert_eq!(d[1].id_note, None);
    }

    /// Old API result stored through a pointer or destructured: kept with a reason.
    #[test]
    fn old_api_pointer_store_gets_note() {
        let (d, u) = run(r#"
unsafe fn f(slot: *mut i32) {
    *slot = smashline::clone_weapon("link", *WEAPON_KIND_LINK_SWORD_BEAM, "samus", "a", false);
    let (x, y) = smashline::clone_weapon("link", *WEAPON_KIND_LINK_SWORD_BEAM, "samus", "b", false);
}
"#);
        assert!(u.is_empty(), "{u:?}");
        assert_eq!(d.len(), 2);
        for decl in &d {
            assert!(decl.id_bindings.is_empty());
            assert!(decl
                .id_note
                .as_deref()
                .is_some_and(|n| n.contains("does not follow")));
        }
    }

    /// Empoleon-Moveset · src/lib.rs:408 (bare statement, full path kind, `true`).
    #[test]
    fn use_original_code_true() {
        let (d, u) = run(r#"
fn f() {
    smashline::clone_weapon("link", *smash::lib::lua_const::WEAPON_KIND_LINK_SWORD_BEAM, "falco", "cannon", true);
}
"#);
        assert!(u.is_empty(), "{u:?}");
        assert_eq!(d.len(), 1);
        assert!(d[0].use_original_code);
        assert_eq!(d[0].src_kind, "WEAPON_KIND_LINK_SWORD_BEAM");
        assert!(d[0].id_bindings.is_empty());
        assert_eq!(d[0].id_note, None);
    }

    /// Shy-Guy-Moveset · .temp/src(goomba)/goomba/redshell/mod.rs:9 and vars.rs:90.
    #[test]
    fn const_indirection() {
        let (d, u) = run_many(&[
            (
                "vars.rs",
                r#"
pub const REDSHELL_ORIGIN: &str = "buddy";
pub const REDSHELL_KIND: smash::lib::LuaConst = smash::lib::lua_const::WEAPON_KIND_BUDDY_PAD;
"#,
            ),
            (
                "redshell.rs",
                r#"
fn install_hook() {
    unsafe {
        FIGHTER_GOOMBA_GENERATE_ARTICLE_REDSHELL += FIGHTER_GOOMBA_GENERATE_ARTICLE_LAST +
     smashline::clone_weapon(REDSHELL_ORIGIN, *REDSHELL_KIND,
"pichu", "redshell",false);
    }
}
"#,
            ),
        ]);
        assert!(u.is_empty(), "{u:?}");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].src_owner.as_deref(), Some("buddy"));
        assert_eq!(d[0].src_kind, "WEAPON_KIND_BUDDY_PAD");
        assert_eq!(
            d[0].id_bindings,
            ["FIGHTER_GOOMBA_GENERATE_ARTICLE_REDSHELL"]
        );
        assert_eq!(d[0].file, PathBuf::from("redshell.rs"));
    }

    /// Sceptile-Moveset · plugin/src/sceptile/articles/registration.rs:146 (new API in a loop over a
    /// literal table; the corpus passes the pairs as helper arguments, a loop is the equivalent).
    #[test]
    fn literal_table_loop() {
        let (d, u) = run(r#"
fn register() {
    for (name, source) in [("leafstorm", *WEAPON_KIND_RIDLEY_BREATH), ("stick", *WEAPON_KIND_KROOL_IRONBALL)] {
        let clone = smashline::clone_weapon("mario", name, source, false);
        STORE.get_or_init(|| clone.generate_id);
    }
}
"#);
        assert!(u.is_empty(), "{u:?}");
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].new_name, "leafstorm");
        assert_eq!(d[0].src_kind, "WEAPON_KIND_RIDLEY_BREATH");
        assert_eq!(d[1].new_name, "stick");
        assert_eq!(d[1].src_kind, "WEAPON_KIND_KROOL_IRONBALL");
        assert_eq!(d[0].new_owner, "mario");
        assert_eq!(d[1].id_bindings, ["STORE"]);
    }

    /// Latios-Moveset · src/lib.rs:555.
    #[test]
    fn commented_out_is_ignored() {
        let (d, u) = run(r#"
fn f() {
    //FIGHTER_SAMUS_GENERATE_ARTICLE_LATIBALL += smashline::clone_weapon("miiswordsman", *WEAPON_KIND_MIISWORDSMAN_WAVE, "samus", "latiball", false);
    /* clone_weapon("a", *WEAPON_KIND_A_B, "c", "d", false); */
}
"#);
        assert!(d.is_empty() && u.is_empty(), "{d:?} {u:?}");
    }

    #[test]
    fn unresolvable_reports_reason() {
        let (d, u) = run(r#"
fn f() {
    clone_weapon(owner_fn(), *WEAPON_KIND_LINK_SWORD_BEAM, "samus", "x", false);
}
"#);
        assert!(d.is_empty());
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].line, 3);
        assert!(u[0].text.contains("owner_fn()"), "{}", u[0].text);
        assert!(!u[0].reason.is_empty());
    }

    #[test]
    fn unparsable_file_reports_each_call_site() {
        let (d, u) = run("fn broken( {\n  clone_weapon(\"a\", *WEAPON_KIND_A_B, \"c\", \"d\", false);\n  // clone_weapon(\"no\")\n  x = clone_weapon(1);\n");
        assert!(d.is_empty());
        assert_eq!(u.len(), 2, "{u:?}");
        assert_eq!(u[0].line, 2);
        assert_eq!(u[1].line, 4);
        assert!(u.iter().all(|c| c.reason == "the file could not be parsed"));
    }

    #[test]
    fn wrong_argument_count_is_unresolved() {
        let (d, u) = run(r#"fn f() { clone_weapon("a", "b"); }"#);
        assert!(d.is_empty());
        assert_eq!(u.len(), 1);
    }

    /// Master-Chief-Moveset · Master Chief-rs/src/chief/master/weapons/mod.rs:7 (bare old-API call)
    /// and src/chief/consts.rs:38 (`pub const FIGHTER_MASTER_GENERATE_ARTICLE_PLASMASHOT:i32 = 0x9;`),
    /// tier 1, smashline2. The scripts spawn it by that const.
    #[test]
    fn bare_call_binds_the_conventional_const() {
        let (d, u) = run_many(&[
            (
                "weapons.rs",
                r#"pub fn install() {
    clone_weapon("koopa", *WEAPON_KIND_KOOPA_BREATH, "master", "plasmashot", false);
}"#,
            ),
            (
                "consts.rs",
                "pub mod weapon_kind_ex {
    pub const FIGHTER_MASTER_GENERATE_ARTICLE_PLASMASHOT:i32 = 0x9;
}
",
            ),
        ]);
        assert!(u.is_empty(), "{u:?}");
        assert_eq!(
            d[0].id_bindings,
            ["FIGHTER_MASTER_GENERATE_ARTICLE_PLASMASHOT"]
        );
        assert_eq!(d[0].id_note, None);
        // Without the const, a bare call still has no id and no note (its id is discarded).
        let (d, _) = run(
            r#"fn f() { clone_weapon("koopa", *WEAPON_KIND_KOOPA_BREATH, "master", "plasmashot", false); }"#,
        );
        assert!(d[0].id_bindings.is_empty());
        assert_eq!(d[0].id_note, None);
    }

    /// Sceptile-Moveset · plugin/src/sceptile/articles/registration.rs:144-173 (tier 1, smashline2):
    /// a nested `unsafe fn clone(name, source, generate_id: *mut i32, weapon_kind: *mut i32)`
    /// wraps `clone_weapon("mario", name, source, false)` and is called once per article with
    /// `std::ptr::addr_of_mut!(FIGHTER_MARIO_GENERATE_ARTICLE_..)`.
    #[test]
    fn helper_calls_are_one_clone_each() {
        let (d, u) = run(r#"
pub static mut FIGHTER_MARIO_GENERATE_ARTICLE_LEAF_STORM: i32 = -1;
pub static mut WEAPON_KIND_MARIO_LEAF_STORM: i32 = -1;
pub static mut FIGHTER_MARIO_GENERATE_ARTICLE_STICK: i32 = -1;
pub static mut WEAPON_KIND_MARIO_STICK: i32 = -1;
pub(in crate::sceptile) fn register_clones() {
    unsafe fn clone(name: &str, source: i32, generate_id: *mut i32, weapon_kind: *mut i32) {
        let clone = smashline::clone_weapon("mario", name, source, false);
        *generate_id = clone.generate_id;
        *weapon_kind = clone.weapon_id;
    }

    unsafe {
        clone("leafstorm", *WEAPON_KIND_RIDLEY_BREATH,
            std::ptr::addr_of_mut!(FIGHTER_MARIO_GENERATE_ARTICLE_LEAF_STORM), std::ptr::addr_of_mut!(WEAPON_KIND_MARIO_LEAF_STORM));
        clone("stick", *WEAPON_KIND_KROOL_IRONBALL,
            std::ptr::addr_of_mut!(FIGHTER_MARIO_GENERATE_ARTICLE_STICK), std::ptr::addr_of_mut!(WEAPON_KIND_MARIO_STICK));
    }
}
"#);
        assert!(u.is_empty(), "{u:?}");
        assert_eq!(d.len(), 2, "{d:#?}");
        assert_eq!(
            (d[0].new_owner.as_str(), d[0].new_name.as_str()),
            ("mario", "leafstorm")
        );
        assert_eq!(d[0].src_kind, "WEAPON_KIND_RIDLEY_BREATH");
        assert_eq!(
            d[0].id_bindings,
            ["FIGHTER_MARIO_GENERATE_ARTICLE_LEAF_STORM"]
        );
        assert_eq!(d[0].kind_bindings, ["WEAPON_KIND_MARIO_LEAF_STORM"]);
        assert_eq!(d[0].id_note, None);
        assert_eq!(d[0].line, 14, "placed at the call");
        assert_eq!(d[1].new_name, "stick");
        assert_eq!(d[1].id_bindings, ["FIGHTER_MARIO_GENERATE_ARTICLE_STICK"]);
        assert_eq!(d[1].line, 16);
        // A helper called with arguments it can't read reports at its call.
        let (d, u) = run(r#"
fn reg(n: &str) {
    fn clone(name: &str) { smashline::clone_weapon("mario", name, *WEAPON_KIND_MARIO_FIREBALL, false); }
    clone(n);
}
"#);
        assert!(d.is_empty());
        assert_eq!(u.len(), 1, "{u:?}");
    }

    /// C-1: a helper parameter whose argument is an unknown name of its own must not loop.
    /// Shapes: `fn reg(name: &str) { clone(name, ..) }`, a `CLONES.iter()` loop whose names match
    /// the helper's parameters, and a two-helper chain `a → b → a`.
    #[test]
    fn self_named_helper_arguments_are_unknown_not_a_loop() {
        let (d, u) = run(r#"
fn clone(name: &str, kind: i32) { smashline::clone_weapon("mario", name, kind, false); }
fn reg(name: &str) { clone(name, *WEAPON_KIND_MARIO_FIREBALL); }
"#);
        assert!(d.is_empty(), "{d:#?}");
        assert_eq!(u.len(), 1, "{u:?}");
        assert!(
            u[0].reason.contains("name") && u[0].reason.contains("helper `clone`"),
            "{}",
            u[0].reason
        );
        assert_eq!(u[0].line, 3, "reported at the call");

        let (d, u) = run(r#"
fn clone(name: &str, kind: i32) { smashline::clone_weapon("mario", name, kind, false); }
fn reg() { for (name, kind) in CLONES.iter() { clone(name, *kind); } }
"#);
        assert!(d.is_empty(), "{d:#?}");
        assert_eq!(u.len(), 1, "{u:?}");
        assert!(!u[0].reason.is_empty());

        let (d, u) = run(r#"
fn a(x: &str) { b(x); }
fn b(y: &str) { for x in [y] { smashline::clone_weapon("mario", x, *WEAPON_KIND_MARIO_FIREBALL, false); } a(y); }
fn reg(y: &str) { a(y); }
"#);
        assert!(d.is_empty(), "{d:#?}");
        assert!(
            !u.is_empty() && u.iter().all(|u| !u.reason.is_empty()),
            "{u:?}"
        );
    }

    /// I-2: a helper in `util.rs` called from a fighter's module, by a path, and a `self::` call.
    #[test]
    fn helpers_are_found_across_files_and_paths() {
        let util = r#"pub fn clone_article(name: &str, kind: i32, id: *mut i32) {
    let c = smashline::clone_weapon("mario", name, kind, false);
    unsafe { *id = c.generate_id; }
}"#;
        let fighter = r#"pub static mut FIGHTER_MARIO_GENERATE_ARTICLE_LEAF: i32 = -1;
pub fn install() {
    crate::util::clone_article("leaf", *WEAPON_KIND_RIDLEY_BREATH, std::ptr::addr_of_mut!(FIGHTER_MARIO_GENERATE_ARTICLE_LEAF));
}"#;
        let (d, u) = run_many(&[("util.rs", util), ("mario/mod.rs", fighter)]);
        assert!(u.is_empty(), "{u:?}");
        assert_eq!(d.len(), 1, "{d:#?}");
        assert_eq!(d[0].new_name, "leaf");
        assert_eq!(d[0].file, PathBuf::from("mario/mod.rs"));
        assert_eq!(d[0].line, 3);
        assert_eq!(d[0].id_bindings, ["FIGHTER_MARIO_GENERATE_ARTICLE_LEAF"]);

        let (d, u) = run(r#"
fn clone(name: &str) { smashline::clone_weapon("mario", name, *WEAPON_KIND_MARIO_FIREBALL, false); }
fn reg() { self::clone("ball"); }
"#);
        assert!(u.is_empty(), "{u:?}");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].new_name, "ball");

        // Never called where Visionary can follow it: listed, not dropped.
        let (d, u) = run_many(&[("util.rs", util)]);
        assert!(d.is_empty());
        assert_eq!(u.len(), 1, "{u:?}");
        assert!(
            u[0].reason.contains(
                "made inside helper `clone_article`, whose callers Visionary couldn't follow"
            ),
            "{u:?}"
        );
        assert_eq!(
            (u[0].file.clone(), u[0].line),
            (PathBuf::from("util.rs"), 2)
        );

        // A function sharing the helper's name in another file is still read.
        let other = r#"fn clone_article() { let _ = smashline::clone_weapon("mario", "orb", *WEAPON_KIND_MARIO_FIREBALL, false); }"#;
        let (d, _) = run_many(&[("util.rs", util), ("other.rs", other)]);
        assert!(d.iter().any(|d| d.new_name == "orb"), "{d:#?}");
        // `Arc::clone(&x)` is never a helper call.
        let (d, u) = run(r#"
fn clone(name: &str) { smashline::clone_weapon("mario", name, *WEAPON_KIND_MARIO_FIREBALL, false); }
fn reg(x: &Arc<i32>) { let _ = Arc::clone(x); clone("ball"); }
"#);
        assert_eq!(d.len(), 1, "{d:#?} {u:?}");
        assert!(u.is_empty(), "{u:?}");
    }

    #[test]
    fn module_qualified_helper_call_does_not_inline_local_same_name() {
        let local_file = r#"
fn make(name: &str) {
    smashline::clone_weapon("mario", name, *WEAPON_KIND_MARIO_FIREBALL, false);
}
pub fn install() {
    other::make("ice");
}
"#;
        let other_file = r#"
pub fn make(name: &str) {
    smashline::clone_weapon("luigi", name, *WEAPON_KIND_LUIGI_FIREBALL, false);
}
"#;
        let (d, u) = run_many(&[("a.rs", local_file), ("other.rs", other_file)]);
        assert_eq!(d.len(), 1, "{d:#?}");
        assert_eq!(d[0].new_name, "ice");
        assert_eq!(d[0].src_kind, "WEAPON_KIND_LUIGI_FIREBALL");
        // Local `make` was never called, so it stays reported as an unfollowed helper rather than
        // being silently inlined in place of `other::make`.
        assert!(
            u.iter().any(|x| x.file == PathBuf::from("a.rs")),
            "uncalled local helper should be reported: {u:?}"
        );
    }

    #[test]
    fn ambiguous_helper_call_reports_unresolved_even_when_helpers_were_inlined() {
        let a = r#"
pub fn make(name: &str) {
    smashline::clone_weapon("mario", name, *WEAPON_KIND_MARIO_FIREBALL, false);
}
pub fn reg_a() { make("a_ball"); }
"#;
        let b = r#"
pub fn make(name: &str) {
    smashline::clone_weapon("luigi", name, *WEAPON_KIND_LUIGI_FIREBALL, false);
}
pub fn reg_b() { make("b_ball"); }
"#;
        let c = r#"
pub fn reg_c() { make("ambig_ball"); }
"#;
        let (d, u) = run_many(&[("a.rs", a), ("b.rs", b), ("c.rs", c)]);
        assert_eq!(d.len(), 2, "{d:#?}");
        assert!(
            u.iter().any(|x| x.file == PathBuf::from("c.rs") && x.reason.contains("ambiguous")),
            "ambiguous call in c.rs must be reported even when both helpers were inlined elsewhere: {u:?}"
        );
    }

    #[test]
    fn crlf_sources_work() {
        let src = "fn f() {\r\n    X += smashline::clone_weapon(\"link\", *WEAPON_KIND_LINK_SWORD_BEAM,\r\n        \"samus\", \"a\", false);\r\n}\r\n";
        let (d, u) = run(src);
        assert!(u.is_empty(), "{u:?}");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].line, 2);
    }
}
