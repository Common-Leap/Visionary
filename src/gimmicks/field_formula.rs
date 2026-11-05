//! Plain-language captions for controller formulas shown in the node workspace.

use egui::Color32;

use super::behavior::{BehaviorGraph, Expr, Predicate};
use super::field_values::{self, ExprPath, PredicatePath, ValueRoot};

const VALUE: Color32 = Color32::from_rgb(91, 203, 205);
const CONDITION: Color32 = Color32::from_rgb(190, 155, 229);
const ERROR: Color32 = Color32::from_rgb(235, 125, 110);

pub(super) fn describe_expression(
    graph: &BehaviorGraph,
    root: &ValueRoot,
    path: &ExprPath,
) -> (String, String, Color32) {
    match field_values::inspect_preflighted_expression(graph, root, path) {
        Ok(value) => {
            let (title, detail) = match value {
                Expr::StatusFrame | Expr::MotionFrame => {
                    ("Phase frame", super::graph_view::source_text(value))
                }
                Expr::StickX { .. }
                | Expr::StickY
                | Expr::RandomInt { .. }
                | Expr::RandomFloat { .. } => ("Controller source", format!("{value:?}")),
                Expr::PendingDamage => (
                    "Pending damage",
                    "Positive native units from the owning damage root".into(),
                ),
                Expr::Const(value) => ("Number", format!("{value:?}")),
                Expr::Var(name) => ("Read variable", name.clone()),
                Expr::TimerRemaining(name) => ("Timer remaining", name.clone()),
                Expr::EventField { event, field } => (
                    "Event field",
                    format!(
                        "{} · {}",
                        event.chars().take(48).collect::<String>(),
                        field.chars().take(48).collect::<String>()
                    ),
                ),
                Expr::NativeFloat(source) | Expr::NativeInt(source) => (
                    "Read native value",
                    crate::move_nodes::view::native_value_detail(
                        source,
                        if matches!(value, Expr::NativeInt(_)) {
                            super::native_inputs::NativeKind::Int
                        } else {
                            super::native_inputs::NativeKind::Float
                        },
                    ),
                ),
                Expr::Add(..) => ("Add", "Left + Right".into()),
                Expr::Sub(..) => ("Subtract", "Left − Right".into()),
                Expr::Mul(..) => ("Multiply", "Left × Right".into()),
            };
            let side = path
                .0
                .last()
                .map_or("Value", |side| if *side == 0 { "Left" } else { "Right" });
            (title.into(), format!("{side} · {detail}"), VALUE)
        }
        Err(error) => ("Missing value".into(), error, ERROR),
    }
}

pub(super) fn describe_predicate(
    graph: &BehaviorGraph,
    transition: usize,
    path: &PredicatePath,
) -> (String, String, Color32) {
    match field_values::inspect_preflighted_predicate(graph, transition, path) {
        Ok(value) => {
            let (title, detail) = match value {
                Predicate::DamageIsProjectile => ("Projectile at contact", format!("{value:?}")),
                Predicate::InStatus { .. }
                | Predicate::InMotion { .. }
                | Predicate::SituationChanged
                | Predicate::MotionEnded
                | Predicate::CancelEnabled
                | Predicate::TermEnabled { .. }
                | Predicate::Situation { .. } => (
                    "Phase condition",
                    super::graph_view::phase_predicate_text(value),
                ),
                Predicate::Button { .. } => ("Button", format!("{value:?}")),
                Predicate::Flag(name) => ("Flag condition", name.clone()),
                Predicate::EventFlag { event, field } => (
                    "Event flag",
                    format!(
                        "{} · {}",
                        event.chars().take(48).collect::<String>(),
                        field.chars().take(48).collect::<String>()
                    ),
                ),
                Predicate::DamageStatusIs(source) => ("Status at contact", source.label().into()),
                Predicate::NativeFlag(source) => (
                    "Read native value",
                    crate::move_nodes::view::native_value_detail(
                        source,
                        super::native_inputs::NativeKind::Flag,
                    ),
                ),
                Predicate::Expired(name) => ("Timer expired", name.clone()),
                Predicate::Compare { op, .. } => ("Compare", format!("Left {op} Right")),
                Predicate::Not(_) => ("Not", "Invert condition".into()),
                Predicate::And(..) => ("All", "Both conditions must hold".into()),
                Predicate::Or(..) => ("Any", "Either condition may hold".into()),
            };
            (title.into(), detail, CONDITION)
        }
        Err(error) => ("Missing condition".into(), error, ERROR),
    }
}
