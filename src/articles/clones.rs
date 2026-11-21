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
        decls: Vec::new(),
        unresolved: Vec::new(),
    };
    for (file, text, ast) in parsed {
        out.file = file.clone();
        out.lines = text.lines().collect();
        match ast {
            Some(ast) => syn::visit::Visit::visit_file(&mut out, &ast),
            None => out.text_scan(text),
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
            syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => {
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
    env: Vec<(String, syn::Expr)>,
    decls: Vec<CloneDecl>,
    unresolved: Vec<UnresolvedClone>,
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

impl Reader<'_> {
    fn line_of(span: proc_macro2::Span) -> usize {
        span.start().line
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
        let unresolved = UnresolvedClone {
            file: self.file.clone(),
            line: Self::line_of(span),
            text: self.text_of(span),
            reason: reason.into(),
        };
        self.unresolved.push(unresolved);
    }

    fn lookup_env(&self, name: &str) -> Option<&syn::Expr> {
        self.env.iter().rev().find(|(n, _)| n == name).map(|(_, e)| e)
    }

    /// A string argument: a literal, a loop variable bound to one, or a `const &str`.
    fn string_arg(&self, expr: &syn::Expr, what: &str) -> Result<String, String> {
        match strip(expr) {
            syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => Ok(s.value()),
            syn::Expr::Path(p) if p.path.segments.len() == 1 => {
                let name = p.path.segments[0].ident.to_string();
                if let Some(bound) = self.lookup_env(&name) {
                    return self.string_arg(bound, what);
                }
                self.named.string(&name).map(str::to_string).ok_or_else(|| {
                    format!("the {what} is `{name}`, which is not a string literal or a `const &str` in the project")
                })
            }
            _ => Err(format!("the {what} is computed, not a string literal or a `const &str`")),
        }
    }

    /// A weapon kind argument, reduced to its bare `WEAPON_KIND_…` name.
    fn kind_arg(&self, expr: &syn::Expr) -> Result<String, String> {
        let Some(name) = path_name(expr) else {
            return Err("the source weapon kind is computed, not a `WEAPON_KIND_…` constant".into());
        };
        if let Some(bound) = self.lookup_env(&name) {
            return self.kind_arg(bound);
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
            syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Bool(b), .. }) => Ok(b.value),
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
            n => Err(format!("clone_weapon takes 4 or 5 arguments, this call has {n}")),
        };
        match built {
            Ok(mut decl) => {
                decl.file = self.file.clone();
                decl.line = Self::line_of(call.span());
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
        let Some(call) = find_clone_call(value) else { return false };
        let id = self.resolve(call, false);
        if let (Some(index), Some(name)) = (id, target_name(target)) {
            self.decls[index].id_bindings.push(name);
            self.decls[index].id_note = None;
        }
        true
    }

    /// `A.get_or_init(|| clone_weapon(..).generate_id)` / `A = clone_weapon(..).weapon_id`.
    fn inline_store(&mut self, target: &syn::Expr, value: &syn::Expr) -> bool {
        let Some((call, field)) = inline_field(value) else { return false };
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
        let Some(init) = &local.init else { return false };
        let Some(call) = find_clone_call(&init.expr) else { return false };
        let var = match &local.pat {
            syn::Pat::Ident(p) => Some(p.ident.to_string()),
            _ => None,
        };
        let Some(index) = self.resolve(call, false) else { return true };
        if let Some(var) = var {
            // The old signature returns the id itself, so the variable is the id.
            let direct = call.args.len() == 5;
            let mut finder = BindingFinder { var, direct, aliases: Vec::new(), ids: Vec::new(), kinds: Vec::new() };
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
        if is_clone_call(call) {
            self.resolve(call, false);
            return;
        }
        syn::visit::visit_expr_call(self, call);
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
                (n, syn::Expr::Tuple(t)) if matches!(*node.pat, syn::Pat::Tuple(_)) => t.elems.len() == n.len(),
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
        let mut probe = Probe { var: &self.var, direct: self.direct, aliases: &self.aliases, found: None };
        syn::visit::Visit::visit_expr(&mut probe, expr);
        probe.found
    }

    fn store(&mut self, target: &syn::Expr, value: &syn::Expr) {
        let (Some(name), Some(field)) = (target_name(target), self.field_of(value)) else { return };
        let list = if field == "id" { &mut self.ids } else { &mut self.kinds };
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
        run_dirs(files, &["mario", "miiswordsman", "link", "kingteresa", "mii", "buddy"])
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
        assert_eq!(d[0].id_bindings, ["FIGHTER_SAMUS_GENERATE_ARTICLE_LATIVOLT"]);
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
        assert_eq!(d[0].id_bindings, ["FIGHTER_KINGTERESA_GENERATE_ARTICLE_FIREBALL"]);
        assert_eq!(d[0].kind_bindings, ["WEAPON_KIND_KINGTERESA_FIREBALL"]);
        // Bindings never leak to the next clone; `.set(..)` counts like get_or_init.
        assert_eq!(d[1].id_bindings, ["FIGHTER_KINGTERESA_GENERATE_ARTICLE_PORTRAIT"]);
        assert!(d[1].kind_bindings.is_empty());
        // No fighter named dolly in the list, so the owner is unknown, not guessed.
        assert_eq!(d[1].src_owner, None);
    }

    /// Longest matching fighter wins: both `PTRAINER_` and `PTRAINER_LOW_` prefix the kind
    /// (synthetic dir names; a first-match or shortest-match would pick `ptrainer`).
    #[test]
    fn new_api_source_owner_prefers_longest() {
        let (d, _) = run_dirs(
            &[("lib.rs", r#"fn f() { let a = clone_weapon("x", "y", *WEAPON_KIND_PTRAINER_LOW_BALL, false); }"#)],
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
            assert!(decl.id_note.as_deref().is_some_and(|n| n.contains("does not follow")));
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
        assert_eq!(d[0].id_bindings, ["FIGHTER_GOOMBA_GENERATE_ARTICLE_REDSHELL"]);
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
            ("weapons.rs", r#"pub fn install() {
    clone_weapon("koopa", *WEAPON_KIND_KOOPA_BREATH, "master", "plasmashot", false);
}"#),
            ("consts.rs", "pub mod weapon_kind_ex {
    pub const FIGHTER_MASTER_GENERATE_ARTICLE_PLASMASHOT:i32 = 0x9;
}
"),
        ]);
        assert!(u.is_empty(), "{u:?}");
        assert_eq!(d[0].id_bindings, ["FIGHTER_MASTER_GENERATE_ARTICLE_PLASMASHOT"]);
        assert_eq!(d[0].id_note, None);
        // Without the const, a bare call still has no id and no note (its id is discarded).
        let (d, _) = run(r#"fn f() { clone_weapon("koopa", *WEAPON_KIND_KOOPA_BREATH, "master", "plasmashot", false); }"#);
        assert!(d[0].id_bindings.is_empty());
        assert_eq!(d[0].id_note, None);
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
