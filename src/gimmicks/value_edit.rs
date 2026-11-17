//! One inline editor for every numeric slot in a controller: a number, a
//! variable, a game value, the hit's damage, a random roll, or a sum of those.

use eframe::egui::{self, Ui};

use super::behavior::{BehaviorGraph, ConstValue, Expr, Predicate, VarType};
use super::native_inputs::{NativeInput, NativeKind};

/// What the transition owning a value can read.
#[derive(Clone, Copy, Default)]
pub(crate) struct ValueScope {
    /// Owned only by the every-frame hook: input and status phase are sampled.
    pub every_frame: bool,
    /// Owned by a deal/take damage hook: the hit's damage is known.
    pub damage: bool,
    /// Runs as a phase of an owned fighter status, where status commands apply.
    pub status_phase: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Source {
    Number,
    Variable,
    TimerLeft,
    Game,
    Damage,
    Random,
    StickForward,
    StickUp,
    StatusFrame,
    MotionFrame,
    Articles,
    Projectile,
    Other,
}

impl Source {
    fn of(expr: &Expr) -> Self {
        match expr {
            Expr::Const(_) => Self::Number,
            Expr::Var(_) => Self::Variable,
            Expr::TimerRemaining(_) => Self::TimerLeft,
            Expr::NativeInt(NativeInput::ArticleCount { .. }) => Self::Articles,
            Expr::NativeFloat(input) | Expr::NativeInt(input)
                if input.projectile_article().is_some() =>
            {
                Self::Projectile
            }
            Expr::NativeFloat(_) | Expr::NativeInt(_) => Self::Game,
            Expr::PendingDamage => Self::Damage,
            Expr::RandomInt { .. } | Expr::RandomFloat { .. } => Self::Random,
            Expr::StickX { .. } => Self::StickForward,
            Expr::StickY => Self::StickUp,
            Expr::StatusFrame => Self::StatusFrame,
            Expr::MotionFrame => Self::MotionFrame,
            _ => Self::Other,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Number => "Number",
            Self::Variable => "Variable",
            Self::TimerLeft => "Timer left",
            Self::Game => "Game value",
            Self::Projectile => "Projectile value",
            Self::Damage => "Hit damage",
            Self::Random => "Random",
            Self::StickForward => "Stick forward",
            Self::StickUp => "Stick up",
            Self::StatusFrame => "Frames in status",
            Self::MotionFrame => "Animation frame",
            Self::Articles => "Article count",
            Self::Other => "Formula",
        }
    }
}

impl ValueScope {
    /// Input and status phase are sampled only by the frame tick, so only a
    /// transition it runs alone may read them; damage only on damage hooks.
    pub(crate) fn of_transition(graph: &BehaviorGraph, transition: usize) -> Self {
        use super::hooks::HookName;
        let plan = &graph.emission_plan;
        let owners: Vec<HookName> = plan
            .bindings
            .iter()
            .filter(|binding| binding.transitions.contains(&transition))
            .map(|binding| binding.hook)
            .collect();
        Self {
            every_frame: owners == [HookName::OnChargeTick]
                && !plan
                    .event_bindings
                    .iter()
                    .any(|binding| binding.transitions.contains(&transition)),
            damage: matches!(
                owners.as_slice(),
                [HookName::OnDealDamage
                    | HookName::OnTakeDamage
                    | HookName::OnAttackShielded
                    | HookName::OnShieldHit]
            ),
            status_phase: super::status_model::phase(
                graph
                    .states
                    .iter()
                    .filter_map(|state| state.status.as_ref()),
                transition,
            )
            .is_some(),
        }
    }
}

/// The scalar kind an expression evaluates to, when it is a plain number.
pub(crate) fn kind_of(graph: &BehaviorGraph, expr: &Expr) -> Option<VarType> {
    match expr {
        Expr::Const(ConstValue::Float(_))
        | Expr::NativeFloat(_)
        | Expr::PendingDamage
        | Expr::RandomFloat { .. }
        | Expr::StickX { .. }
        | Expr::StickY
        | Expr::MotionFrame => Some(VarType::Float),
        Expr::Const(ConstValue::Int(_))
        | Expr::NativeInt(_)
        | Expr::RandomInt { .. }
        | Expr::StatusFrame
        | Expr::TimerRemaining(_) => Some(VarType::Int),
        Expr::Var(name) => graph
            .vars
            .iter()
            .find(|var| var.name == *name)
            .map(|var| var.kind),
        Expr::Add(left, _) | Expr::Sub(left, _) | Expr::Mul(left, _) => kind_of(graph, left),
        Expr::Const(ConstValue::Flag(_)) | Expr::EventField { .. } => None,
    }
}

pub(crate) fn zero(kind: VarType) -> Expr {
    Expr::Const(if kind == VarType::Float {
        ConstValue::Float(0.0)
    } else {
        ConstValue::Int(0)
    })
}

fn native_kind(kind: VarType) -> NativeKind {
    if kind == VarType::Float {
        NativeKind::Float
    } else {
        NativeKind::Int
    }
}

fn sources(graph: &BehaviorGraph, fighter: &str, kind: VarType, scope: ValueScope) -> Vec<Source> {
    let float = kind == VarType::Float;
    let mut out = vec![Source::Number];
    if graph.vars.iter().any(|var| var.kind == kind) {
        out.push(Source::Variable);
    }
    if !float && graph.vars.iter().any(|var| var.kind == VarType::Timer) {
        out.push(Source::TimerLeft);
    }
    if !NativeInput::catalog_for(fighter, native_kind(kind)).is_empty() {
        out.push(Source::Game);
    }
    if scope.every_frame
        && NativeInput::catalog_for(fighter, native_kind(kind))
            .iter()
            .any(|input| input.projectile_article().is_some())
    {
        out.push(Source::Projectile);
    }
    if !float && first_article(fighter).is_some() {
        out.push(Source::Articles);
    }
    if float && scope.damage {
        out.push(Source::Damage);
    }
    out.push(Source::Random);
    if scope.every_frame {
        if float {
            out.extend([Source::StickForward, Source::StickUp, Source::MotionFrame]);
        } else {
            out.push(Source::StatusFrame);
        }
    }
    out
}

fn fresh(graph: &BehaviorGraph, fighter: &str, kind: VarType, source: Source) -> Expr {
    let float = kind == VarType::Float;
    match source {
        Source::Variable => graph
            .vars
            .iter()
            .find(|var| var.kind == kind)
            .map_or_else(|| zero(kind), |var| Expr::Var(var.name.clone())),
        Source::TimerLeft => graph
            .vars
            .iter()
            .find(|var| var.kind == VarType::Timer)
            .map_or_else(|| zero(kind), |var| Expr::TimerRemaining(var.name.clone())),
        Source::Projectile => NativeInput::catalog_for(fighter, native_kind(kind))
            .into_iter()
            .find(|input| input.projectile_article().is_some())
            .map_or_else(
                || zero(kind),
                |input| {
                    if float {
                        Expr::NativeFloat(input)
                    } else {
                        Expr::NativeInt(input)
                    }
                },
            ),
        Source::Game => NativeInput::catalog_for(fighter, native_kind(kind))
            .into_iter()
            .next()
            .map_or_else(
                || zero(kind),
                |input| {
                    if float {
                        Expr::NativeFloat(input)
                    } else {
                        Expr::NativeInt(input)
                    }
                },
            ),
        Source::Damage => Expr::PendingDamage,
        Source::Random if float => Expr::RandomFloat { min: 0.0, max: 1.0 },
        Source::Random => Expr::RandomInt { min: 1, max: 3 },
        Source::StickForward => Expr::StickX {
            relative_to_facing: true,
        },
        Source::StickUp => Expr::StickY,
        Source::StatusFrame => Expr::StatusFrame,
        Source::MotionFrame => Expr::MotionFrame,
        Source::Articles => Expr::NativeInt(NativeInput::ArticleCount {
            fighter: fighter.into(),
            symbol: first_article(fighter).unwrap_or_default(),
        }),
        Source::Number | Source::Other => zero(kind),
    }
}

const OPS: [&str; 3] = ["+", "−", "×"];

/// Flatten a left-nested sum/product into its first term and (op, term) steps.
fn terms(expr: &Expr) -> (Expr, Vec<(usize, Expr)>) {
    let (left, op, right) = match expr {
        Expr::Add(left, right) => (left, 0, right),
        Expr::Sub(left, right) => (left, 1, right),
        Expr::Mul(left, right) => (left, 2, right),
        _ => return (expr.clone(), Vec::new()),
    };
    let (first, mut rest) = terms(left);
    rest.push((op, (**right).clone()));
    (first, rest)
}

fn fold(first: Expr, rest: Vec<(usize, Expr)>) -> Expr {
    rest.into_iter().fold(first, |left, (op, right)| {
        let (left, right) = (Box::new(left), Box::new(right));
        match op {
            0 => Expr::Add(left, right),
            1 => Expr::Sub(left, right),
            _ => Expr::Mul(left, right),
        }
    })
}

/// Edit one numeric value of `kind`. A formula is shown one term per line and
/// evaluated top to bottom: `10`, `× meter`, `+ 5` is (10 × meter) + 5.
pub(crate) fn value_editor(
    ui: &mut Ui,
    fighter: &str,
    graph: &BehaviorGraph,
    expr: &mut Expr,
    kind: VarType,
    scope: ValueScope,
) -> bool {
    let before = expr.clone();
    let (mut first, mut rest) = terms(expr);
    ui.vertical(|ui| {
        ui.horizontal(|ui| term_editor(ui, fighter, graph, &mut first, kind, scope, 0));
        let mut remove = None;
        for (index, (op, term)) in rest.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt(("value_op", index))
                    .width(24.0)
                    .selected_text(OPS[*op])
                    .show_ui(ui, |ui| {
                        for (choice, label) in OPS.into_iter().enumerate() {
                            ui.selectable_value(op, choice, label);
                        }
                    });
                term_editor(ui, fighter, graph, term, kind, scope, index + 1);
                if ui.small_button("Remove").clicked() {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            rest.remove(index);
        }
        if ui
            .small_button("+ Add term")
            .on_hover_text("Add, subtract or multiply by another value")
            .clicked()
        {
            rest.push((0, zero(kind)));
        }
    });
    *expr = fold(first, rest);
    *expr != before
}

fn term_editor(
    ui: &mut Ui,
    fighter: &str,
    graph: &BehaviorGraph,
    expr: &mut Expr,
    kind: VarType,
    scope: ValueScope,
    depth: usize,
) {
    let current = Source::of(expr);
    let mut choice = current;
    egui::ComboBox::from_id_salt(("value_source", depth))
        .width(92.0)
        .selected_text(current.label())
        .show_ui(ui, |ui| {
            for source in sources(graph, fighter, kind, scope) {
                ui.selectable_value(&mut choice, source, source.label());
            }
        });
    if choice != current {
        *expr = fresh(graph, fighter, kind, choice);
    }
    match expr {
        Expr::Const(ConstValue::Float(value)) => {
            ui.add(
                egui::DragValue::new(value)
                    .speed(0.1)
                    .range(-1_000_000.0..=1_000_000.0),
            );
        }
        Expr::Const(ConstValue::Int(value)) => {
            ui.add(egui::DragValue::new(value).range(-1_000_000..=1_000_000));
        }
        Expr::Var(name) => pick_var(ui, graph, name, kind, depth),
        Expr::TimerRemaining(name) => pick_var(ui, graph, name, VarType::Timer, depth),
        Expr::NativeInt(NativeInput::ArticleCount { symbol, .. }) => {
            egui::ComboBox::from_id_salt(("value_article", depth))
                .selected_text(super::editor::article_label(symbol))
                .show_ui(ui, |ui| {
                    for entry in super::registry::entries_for(
                        fighter,
                        super::registry::IdentifierKind::Article,
                    ) {
                        ui.selectable_value(
                            symbol,
                            entry.symbol.into(),
                            super::editor::article_label(entry.symbol),
                        );
                    }
                });
        }
        Expr::NativeFloat(input) | Expr::NativeInt(input) => {
            ui.push_id(("value_native", depth), |ui| {
                if input.projectile_article().is_some() {
                    egui::ComboBox::from_id_salt("projectile_value").selected_text(input.label()).show_ui(ui, |ui| {
                        for candidate in NativeInput::catalog_for(fighter, native_kind(kind)).into_iter().filter(|input| input.projectile_article().is_some()) {
                            let label = candidate.label().to_owned();
                            ui.selectable_value(input, candidate, label);
                        }
                    }).response.on_hover_text("Samples the newest owned live projectile each frame; zero when absent. Prefer charge, counters and flags read by the weapon; some state is overwritten every frame.");
                } else { crate::move_nodes::view::native_source_ui(ui, fighter, input, native_kind(kind)); }
            });
        }
        Expr::RandomFloat { min, max } => {
            ui.add(egui::DragValue::new(min).speed(0.1));
            ui.label("to");
            ui.add(egui::DragValue::new(max).speed(0.1));
        }
        Expr::RandomInt { min, max } => {
            ui.add(egui::DragValue::new(min));
            ui.label("to");
            ui.add(egui::DragValue::new(max));
        }
        Expr::StickX { relative_to_facing } => {
            let mut right = !*relative_to_facing;
            if ui.checkbox(&mut right, "screen right").changed() {
                *relative_to_facing = !right;
            }
        }
        Expr::EventField { event, field } => {
            crate::move_nodes::view::event_read_ui(ui, event, field);
        }
        _ => {}
    }
}

fn first_article(fighter: &str) -> Option<String> {
    super::registry::entries_for(fighter, super::registry::IdentifierKind::Article)
        .next()
        .map(|entry| entry.symbol.into())
}

fn pick_var(ui: &mut Ui, graph: &BehaviorGraph, name: &mut String, kind: VarType, depth: usize) {
    egui::ComboBox::from_id_salt(("value_var", depth))
        .width(92.0)
        .selected_text(name.as_str())
        .show_ui(ui, |ui| {
            for var in graph.vars.iter().filter(|var| var.kind == kind) {
                ui.selectable_value(name, var.name.clone(), &var.name);
            }
        });
}

/// Compact one-line reading of a value, e.g. `meter + hit damage × 2`.
pub(crate) fn expr_text(expr: &Expr) -> String {
    let operand = |expr: &Expr| match expr {
        Expr::Add(..) | Expr::Sub(..) => format!("({})", expr_text(expr)),
        _ => expr_text(expr),
    };
    match expr {
        Expr::Const(ConstValue::Float(value)) => format!("{value}"),
        Expr::Const(ConstValue::Int(value)) => value.to_string(),
        Expr::Const(ConstValue::Flag(value)) => value.to_string(),
        Expr::Var(name) => name.clone(),
        Expr::TimerRemaining(name) => format!("{name} left"),
        Expr::NativeInt(NativeInput::ArticleCount { symbol, .. }) => {
            format!("{} count", super::editor::article_label(symbol))
        }
        Expr::NativeFloat(input) | Expr::NativeInt(input) => input.label().to_lowercase(),
        Expr::PendingDamage => "hit damage".into(),
        Expr::RandomInt { min, max } => format!("random {min}–{max}"),
        Expr::RandomFloat { min, max } => format!("random {min}–{max}"),
        Expr::StickX {
            relative_to_facing: true,
        } => "stick forward".into(),
        Expr::StickX { .. } => "stick right".into(),
        Expr::StickY => "stick up".into(),
        Expr::StatusFrame => "status frame".into(),
        Expr::MotionFrame => "animation frame".into(),
        Expr::EventField { field, .. } => field.clone(),
        Expr::Add(left, right) => format!("{} + {}", expr_text(left), expr_text(right)),
        Expr::Sub(left, right) => format!("{} − {}", expr_text(left), operand(right)),
        Expr::Mul(left, right) => format!("{} × {}", operand(left), operand(right)),
    }
}

/// Compact one-line reading of a condition, e.g. `meter >= 100 and on ground`.
pub(crate) fn predicate_text(predicate: &Predicate) -> String {
    match predicate {
        always if *always == super::graph_view::always() => "always".into(),
        Predicate::Compare { op, left, right } => {
            format!("{} {op} {}", expr_text(left), expr_text(right))
        }
        Predicate::InStatus { .. }
        | Predicate::InMotion { .. }
        | Predicate::SituationChanged
        | Predicate::MotionEnded
        | Predicate::CancelEnabled
        | Predicate::TermEnabled { .. }
        | Predicate::Situation { .. } => {
            super::graph_view::phase_predicate_text(predicate).to_lowercase()
        }
        Predicate::Button { button, edge } => {
            super::graph_view::button_text(*button, *edge).to_lowercase()
        }
        Predicate::Flag(name) => name.clone(),
        Predicate::EventFlag { field, .. } => field.clone(),
        Predicate::NativeFlag(input) => input.label().to_lowercase(),
        Predicate::DamageStatusIs(input) => format!("hit in {}", input.label().to_lowercase()),
        Predicate::DamageIsProjectile => "hit by projectile".into(),
        Predicate::Expired(name) => format!("{name} expired"),
        Predicate::Not(inner) => match inner.as_ref() {
            Predicate::DamageIsProjectile => "hit by fighter".into(),
            Predicate::Flag(name) => format!("not {name}"),
            inner => format!("not ({})", predicate_text(inner)),
        },
        Predicate::And(left, right) => {
            format!("{} and {}", predicate_text(left), predicate_text(right))
        }
        Predicate::Or(left, right) => {
            format!("({} or {})", predicate_text(left), predicate_text(right))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_follow_the_leftmost_term_and_variables() {
        let mut graph = BehaviorGraph::default();
        graph.vars.push(super::super::behavior::VarDef {
            keep_across_form_switch: false,
            name: "meter".into(),
            kind: VarType::Float,
            initial: ConstValue::Float(0.0),
        });
        let sum = Expr::Add(
            Box::new(Expr::Var("meter".into())),
            Box::new(Expr::PendingDamage),
        );
        assert_eq!(kind_of(&graph, &sum), Some(VarType::Float));
        assert_eq!(kind_of(&graph, &Expr::StatusFrame), Some(VarType::Int));
        let int_sources = sources(&graph, "mario", VarType::Int, ValueScope::default());
        assert!(!int_sources.contains(&Source::Variable) && !int_sources.contains(&Source::Damage));
        let float_sources = sources(
            &graph,
            "mario",
            VarType::Float,
            ValueScope {
                damage: true,
                every_frame: false,
                status_phase: false,
            },
        );
        assert!(
            float_sources.contains(&Source::Variable) && float_sources.contains(&Source::Damage)
        );
        assert!(!float_sources.contains(&Source::StickForward));
    }

    #[test]
    fn formulas_flatten_to_terms_and_fold_back_unchanged() {
        let ten = Expr::Const(ConstValue::Float(10.0));
        let formula = Expr::Add(
            Box::new(Expr::Mul(
                Box::new(ten.clone()),
                Box::new(Expr::Var("meter".into())),
            )),
            Box::new(Expr::PendingDamage),
        );
        let (first, rest) = terms(&formula);
        assert_eq!(first, ten);
        assert_eq!(
            rest,
            vec![(2, Expr::Var("meter".into())), (0, Expr::PendingDamage)]
        );
        assert_eq!(fold(first, rest), formula);
        assert_eq!(expr_text(&formula), "10 × meter + hit damage");
        let guard = Predicate::And(
            Box::new(Predicate::Compare {
                op: super::super::behavior::CmpOp::Gte,
                left: Expr::Var("meter".into()),
                right: Expr::Const(ConstValue::Float(100.0)),
            }),
            Box::new(Predicate::Not(Box::new(Predicate::Flag("used".into())))),
        );
        assert_eq!(predicate_text(&guard), "meter >= 100 and not used");
    }
}
