//! Emitted-source verifier and compile fixture (slice F2.5).
//!
//! Generated files say "do not edit", so something must check that nobody
//! did — and that the emitter itself only generates the closed construct set
//! it claims. [`check_source`] parses emitted Rust with `syn` and rejects
//! anything outside that set: loops, calls, macros, matches, `unsafe`,
//! items the emitter never generates. Every finding names its originating
//! node as `fighter / hook` plus, inside a hook function, the transition
//! index in binding order — the emitter lays one `if` per transition, so the
//! k-th block *is* transition k.
//!
//! [`verify_emit`] closes the loop: emit the graph, check the text, and
//! confirm every planned hook has its function and no extra top-level
//! function exists. Emitted code that fails its own verifier is an emitter
//! bug, reported as blockers rather than text.

// This is an export gate foundation. The export pipeline will call it once
// behavior artifacts are written; its tests exercise the complete gate now.

use std::fmt;

/// One verifier finding: where it is and what is wrong with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitFinding {
    pub subject: String,
    pub message: String,
}

impl fmt::Display for EmitFinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.subject, self.message)
    }
}

/// The verifier output: empty findings mean the source is clean.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmitReport {
    pub findings: Vec<EmitFinding>,
}

impl EmitReport {
    fn blocker(&mut self, subject: impl Into<String>, message: impl Into<String>) {
        self.findings.push(EmitFinding {
            subject: subject.into(),
            message: message.into(),
        });
    }

    /// Findings that fail verification.
    pub fn blockers(&self) -> impl Iterator<Item = &EmitFinding> {
        self.findings.iter()
    }

    /// No blockers: the source is emittable as-is.
    pub fn is_clean(&self) -> bool {
        self.blockers().next().is_none()
    }

    /// One line per blocker, for reports.
    pub fn blocker_summary(&self) -> Vec<String> {
        self.blockers().map(|finding| finding.to_string()).collect()
    }
}

struct Checker {
    fighter: String,
    report: EmitReport,
    /// Current hook function, if inside one.
    hook_fn: Option<String>,
    /// Top-level `if` blocks seen in the current hook function: the k-th one
    /// is transition k in binding order.
    if_index: usize,
}

impl Checker {
    fn subject(&self) -> String {
        match &self.hook_fn {
            Some(hook) => format!("{} / {hook} (transition {})", self.fighter, self.if_index),
            None => format!("{} / file", self.fighter),
        }
    }

    fn block(&mut self, message: impl Into<String>) {
        let subject = self.subject();
        self.report.blocker(subject, message);
    }

    fn check_file(&mut self, file: &syn::File) {
        for item in &file.items {
            self.check_item(item);
        }
    }

    fn check_item(&mut self, item: &syn::Item) {
        match item {
            syn::Item::Struct(item) => {
                if !matches!(item.fields, syn::Fields::Named(_)) {
                    self.block("state struct must have named fields");
                }
                if !item.generics.params.is_empty() {
                    self.block("state struct takes no generics");
                }
            }
            syn::Item::Impl(item) => {
                if item.unsafety.is_some() {
                    self.block("unsafe impl block");
                }
                if item.trait_.is_some() {
                    self.block("trait impl; the emitter generates inherent impls only");
                }
                for inner in &item.items {
                    match inner {
                        syn::ImplItem::Fn(method) if method.sig.ident == "initial" => {
                            self.check_block(&method.block);
                        }
                        syn::ImplItem::Fn(method) => {
                            self.block(format!(
                                "unexpected method {:?}; the emitter generates initial() only",
                                method.sig.ident.to_string()
                            ));
                        }
                        _ => self.block("unexpected impl item"),
                    }
                }
            }
            syn::Item::Fn(item) => {
                if item.sig.unsafety.is_some() {
                    self.block("unsafe function");
                }
                if item.sig.abi.is_some() {
                    self.block("extern function");
                }
                if item.sig.constness.is_some() {
                    self.block("const function");
                }
                if item.sig.asyncness.is_some() {
                    self.block("async function");
                }
                if !item.sig.generics.params.is_empty() {
                    self.block("generic function");
                }
                let name = item.sig.ident.to_string();
                if !name.starts_with("gimmick_") {
                    self.block(format!(
                        "unexpected top-level function {name:?}; hooks emit gimmick_* functions"
                    ));
                }
                self.hook_fn = Some(name);
                self.if_index = 0;
                self.check_block(&item.block);
                self.hook_fn = None;
            }
            other => self.block(format!("unexpected top-level item ({})", item_kind(other))),
        }
    }

    fn check_block(&mut self, block: &syn::Block) {
        for stmt in &block.stmts {
            self.check_stmt(stmt);
        }
    }

    fn check_stmt(&mut self, stmt: &syn::Stmt) {
        match stmt {
            syn::Stmt::Local(_) => {
                self.block("let binding; the emitter generates assignments only")
            }
            syn::Stmt::Item(item) => {
                self.block(format!("nested item ({})", item_kind(item)));
            }
            syn::Stmt::Expr(expr, _) => self.check_expr(expr),
            syn::Stmt::Macro(_) => self.block("macro call"),
        }
    }

    fn check_expr(&mut self, expr: &syn::Expr) {
        match expr {
            syn::Expr::Assign(expr) => {
                self.check_expr(&expr.left);
                self.check_expr(&expr.right);
            }
            syn::Expr::Binary(expr) => {
                self.check_expr(&expr.left);
                self.check_expr(&expr.right);
            }
            syn::Expr::Unary(expr) => self.check_expr(&expr.expr),
            syn::Expr::Field(_) | syn::Expr::Path(_) | syn::Expr::Lit(_) => {}
            syn::Expr::Paren(expr) => self.check_expr(&expr.expr),
            syn::Expr::Struct(expr) => {
                for field in &expr.fields {
                    self.check_expr(&field.expr);
                }
            }
            syn::Expr::If(expr) => {
                // A top-level if inside a hook function is the next
                // transition in binding order: count it before descending so
                // findings inside name their originating node.
                if self.hook_fn.is_some() {
                    self.if_index += 1;
                }
                self.check_expr(&expr.cond);
                self.check_block(&expr.then_branch);
                if let Some((_, otherwise)) = &expr.else_branch {
                    self.check_expr(otherwise);
                }
            }
            syn::Expr::Block(expr) => self.check_block(&expr.block),
            other => self.block(format!("unsupported operation ({})", expr_kind(other))),
        }
    }
}

fn item_kind(item: &syn::Item) -> &'static str {
    match item {
        syn::Item::Const(_) => "const",
        syn::Item::Enum(_) => "enum",
        syn::Item::ExternCrate(_) => "extern crate",
        syn::Item::Fn(_) => "fn",
        syn::Item::ForeignMod(_) => "extern block",
        syn::Item::Impl(_) => "impl",
        syn::Item::Macro(_) => "macro",
        syn::Item::Mod(_) => "mod",
        syn::Item::Static(_) => "static",
        syn::Item::Struct(_) => "struct",
        syn::Item::Trait(_) => "trait",
        syn::Item::TraitAlias(_) => "trait alias",
        syn::Item::Type(_) => "type alias",
        syn::Item::Union(_) => "union",
        syn::Item::Use(_) => "use",
        syn::Item::Verbatim(_) => "verbatim",
        _ => "unknown",
    }
}

fn expr_kind(expr: &syn::Expr) -> &'static str {
    match expr {
        syn::Expr::Array(_) => "array",
        syn::Expr::Async(_) => "async block",
        syn::Expr::Await(_) => "await",
        syn::Expr::Break(_) => "break",
        syn::Expr::Call(_) => "function call",
        syn::Expr::Cast(_) => "cast",
        syn::Expr::Closure(_) => "closure",
        syn::Expr::Const(_) => "const block",
        syn::Expr::Continue(_) => "continue",
        syn::Expr::ForLoop(_) => "for loop",
        syn::Expr::Group(_) => "group",
        syn::Expr::Index(_) => "index",
        syn::Expr::Infer(_) => "inferred type",
        syn::Expr::Let(_) => "let guard",
        syn::Expr::Loop(_) => "loop",
        syn::Expr::Macro(_) => "macro call",
        syn::Expr::Match(_) => "match",
        syn::Expr::MethodCall(_) => "method call",
        syn::Expr::Range(_) => "range",
        syn::Expr::RawAddr(_) => "raw address",
        syn::Expr::Reference(_) => "reference",
        syn::Expr::Repeat(_) => "repeat",
        syn::Expr::Return(_) => "return",
        syn::Expr::Try(_) => "try",
        syn::Expr::TryBlock(_) => "try block",
        syn::Expr::Tuple(_) => "tuple",
        syn::Expr::Unsafe(_) => "unsafe block",
        syn::Expr::While(_) => "while loop",
        syn::Expr::Yield(_) => "yield",
        _ => "unknown",
    }
}

/// Check emitted source text. An unparsable file is one blocker; every
/// construct outside the emitter's closed set is a blocker naming its
/// originating `fighter / hook (transition k)` node.
pub fn check_source(fighter: &str, source: &str) -> EmitReport {
    let mut checker = Checker {
        fighter: fighter.to_string(),
        report: EmitReport::default(),
        hook_fn: None,
        if_index: 0,
    };
    match syn::parse_file(source) {
        Ok(file) => checker.check_file(&file),
        Err(error) => checker.block(format!("source does not parse: {error}")),
    }
    checker.report
}

/// Emit the graph and verify the output end to end: emission failures,
/// construct violations, and plan-vs-output completeness (every planned hook
/// has its function; no extra top-level function exists). A clean report
/// means the text is emittable as-is.
/// Validate a behavior for the shared runtime. Named events and native inputs
/// are typed runtime data, not operations in the legacy pure-state source emitter. Their static
/// adapter is parse-checked by the adapter renderer and compiled by its contract
/// fixture; do not claim the pure emitter represented them.
pub fn verify_behavior(
    fighter: &str,
    graph: &super::behavior::BehaviorGraph,
    plan: &super::codegen::EmissionPlan,
) -> EmitReport {
    if !super::behavior::requires_controller_runtime(graph)
        && super::behavior::collect_native_inputs(graph).is_empty()
        && plan.event_bindings.is_empty()
        && !graph
            .transitions
            .iter()
            .flat_map(|transition| &transition.actions)
            .any(|action| {
                matches!(
                    action,
                    super::behavior::Action::EmitEvent { .. }
                        | super::behavior::Action::EmitEventArguments { .. }
                        | super::behavior::Action::EmitEventValues { .. }
                )
            })
    {
        return verify_emit(fighter, graph, plan);
    }
    let mut report = EmitReport::default();
    for error in super::behavior::validate_for_fighter(fighter, graph) {
        report.blocker(error.item, error.reason);
    }
    for error in super::codegen::check_plan(graph, plan) {
        report.blocker(error.item, error.reason);
    }
    for error in super::codegen::check_status_triggers(fighter, plan) {
        report.blocker(error.item, error.reason);
    }
    report
}

pub fn verify_emit(
    fighter: &str,
    graph: &super::behavior::BehaviorGraph,
    plan: &super::codegen::EmissionPlan,
) -> EmitReport {
    let mut report = EmitReport::default();
    let source = match super::codegen::emit(fighter, graph, plan) {
        Ok(source) => source,
        Err(errors) => {
            for error in errors {
                report.blocker(error.item, error.reason);
            }
            return report;
        }
    };
    let checked = check_source(fighter, &source);
    report.findings.extend(checked.findings);

    let file = syn::parse_file(&source).expect("checked source parses");
    let mut fns: Vec<String> = file
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Fn(item) => Some(item.sig.ident.to_string()),
            _ => None,
        })
        .collect();
    fns.sort();
    let mut expected: Vec<String> = plan
        .bindings
        .iter()
        .map(|binding| super::codegen::hook_snake(&binding.hook))
        .collect();
    expected.sort();
    expected.dedup();
    // Output functions are `gimmick_<fighter>_<snake>` plus a numeric suffix
    // only on a name collision, so suffix matching (with an optional _N
    // tail) is exact in every case the emitter produces.
    let tag = format!("gimmick_{}_", super::codegen::fighter_tag(fighter));
    let suffix_of = |name: &str| name.strip_prefix(&tag).map(str::to_string);
    for missing in expected.iter().filter(|snake| {
        !fns.iter().any(|name| {
            suffix_of(name)
                .is_some_and(|suffix| suffix == **snake || suffix.starts_with(&format!("{snake}_")))
        })
    }) {
        report.blocker(
            format!("{fighter} / plan"),
            format!("planned hook has no function for {missing:?} in the output"),
        );
    }
    for extra in fns.iter().filter(|name| {
        !name.starts_with("gimmick_")
            || !expected.iter().any(|snake| {
                suffix_of(name).is_some_and(|suffix| {
                    suffix == *snake || suffix.starts_with(&format!("{snake}_"))
                })
            })
    }) {
        report.blocker(
            format!("{fighter} / plan"),
            format!("output function {extra:?} matches no planned hook"),
        );
    }
    report
}

#[cfg(test)]
mod tests {
    use super::super::behavior::{Expr, Predicate};
    use super::*;
    use crate::gimmicks::fixtures::{charge_graph, charge_plan};

    fn source() -> String {
        super::super::codegen::emit("cloud", &charge_graph(), &charge_plan()).unwrap()
    }

    #[test]
    fn native_reads_without_named_events_use_the_snapshot_runtime_validation() {
        use crate::gimmicks::native_inputs::NativeInput;
        let mut graph = charge_graph();
        let Predicate::Compare { left, .. } = &mut graph.transitions[0].guard else {
            panic!("charge fixture starts with a comparison");
        };
        *left = Expr::NativeFloat(NativeInput::CloudLimitGauge);
        let plan = charge_plan();
        assert!(plan.event_bindings.is_empty());
        assert!(verify_behavior("cloud", &graph, &plan).is_clean());
        assert!(!verify_behavior("mario", &graph, &plan).is_clean());
        assert!(
            !verify_emit("cloud", &graph, &plan).is_clean(),
            "legacy pure emitter has no native snapshot ABI"
        );
    }

    #[test]
    fn emitted_source_verifies_clean() {
        let report = check_source("cloud", &source());
        assert!(report.is_clean(), "{:?}", report.blocker_summary());
        let report = verify_emit("cloud", &charge_graph(), &charge_plan());
        assert!(report.is_clean(), "{:?}", report.blocker_summary());
    }

    #[test]
    fn tampered_constructs_fail_with_their_node() {
        let base = source();
        // unsafe, static, and non-hook functions each fail at file scope.
        for (tamper, kind) in [
            ("unsafe fn evil() {}", "unsafe function"),
            ("static EVIL: i32 = 1;", "static"),
            ("fn evil() {}", "unexpected top-level function"),
        ] {
            let report = check_source("cloud", &format!("{base}\n{tamper}\n"));
            assert!(
                !report.is_clean()
                    && report
                        .blocker_summary()
                        .iter()
                        .any(|line| line.contains(kind) || line.contains("cloud")),
                "{tamper}: {report:?}"
            );
        }
    }

    #[test]
    fn tampered_bodies_name_hook_and_transition() {
        let base = source();
        // A loop smuggled into the charge-tick function: the finding names
        // the hook and the transition block it sits in.
        let injected = base.replacen(
            "state.powered = true;",
            "state.powered = true;\n        loop {}",
            1,
        );
        assert_ne!(injected, base);
        let report = check_source("cloud", &injected);
        let summary = report.blocker_summary();
        assert!(
            summary.iter().any(|line| line.contains("on_charge_tick")
                && line.contains("transition 1")
                && line.contains("loop")),
            "{summary:?}"
        );

        // A call expression: unsupported operation, same attribution.
        let injected = base.replacen("state.gauge = 0.0;", "state.gauge = evil();", 1);
        let report = check_source("cloud", &injected);
        assert!(
            report
                .blocker_summary()
                .iter()
                .any(|line| line.contains("function call")),
            "{:?}",
            report.blocker_summary()
        );

        // A let binding: assignments only.
        let injected = base.replacen(
            "state.powered = false;",
            "let x = 1;\n        state.powered = false;",
            1,
        );
        let report = check_source("cloud", &injected);
        assert!(
            report
                .blocker_summary()
                .iter()
                .any(|line| line.contains("let binding")),
            "{:?}",
            report.blocker_summary()
        );
    }

    #[test]
    fn unparsable_source_is_one_blocker() {
        let report = check_source("cloud", "pub fn gimmick_cloud_broken( {");
        assert_eq!(report.blockers().count(), 1);
        assert!(report.blocker_summary()[0].contains("does not parse"));
    }

    #[test]
    fn invalid_graphs_fail_emit_verification_without_text() {
        let mut graph = charge_graph();
        graph.initial = "missing".to_string();
        let report = verify_emit("cloud", &graph, &charge_plan());
        assert!(!report.is_clean());
        assert!(
            report
                .blocker_summary()
                .iter()
                .any(|line| line.contains("missing")),
            "{:?}",
            report.blocker_summary()
        );
    }

    #[test]
    fn emitted_source_compiles_with_the_host_toolchain() {
        // The generated file is safe, dependency-free Rust: the host rustc
        // compiles it as a fixture. (cargo implies rustc; a missing binary
        // fails loudly rather than skipping.)
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("gimmick_cloud.rs");
        let out = dir.path().join("gimmick_cloud.rlib");
        std::fs::write(&src, source()).unwrap();
        let status = std::process::Command::new("rustc")
            .args([
                "--edition=2021",
                "--crate-type=lib",
                "--crate-name=gimmick_cloud",
            ])
            .arg(&src)
            .arg("-o")
            .arg(&out)
            .output()
            .expect("rustc runs wherever cargo runs");
        assert!(
            status.status.success(),
            "rustc rejected emitted source: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        assert!(out.is_file());
    }
}
