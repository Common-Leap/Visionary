//! Layout of character controllers in the node workspace, and how their parts are described.

use std::collections::BTreeMap;

use egui::Color32;
use serde::{Deserialize, Serialize};

use super::behavior::{Action, BehaviorGraph, VarType};
use super::field_projection::{FieldRef, SourceBindingRef};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "FieldLayoutWire")]
pub struct FieldLayout {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub positions: BTreeMap<String, [f32; 2]>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FieldLayoutWire {
    #[serde(default)]
    positions: BTreeMap<String, [f32; 2]>,
}

impl TryFrom<FieldLayoutWire> for FieldLayout {
    type Error = String;

    fn try_from(wire: FieldLayoutWire) -> Result<Self, Self::Error> {
        if wire.positions.len() > 8_192 {
            return Err("character field layout exceeds 8192 positions".into());
        }
        for (key, point) in &wire.positions {
            FieldRef::from_key(key)?;
            if point
                .iter()
                .any(|value| !value.is_finite() || value.abs() > 100_000.0)
            {
                return Err(format!(
                    "character field position {key:?} is outside its finite coordinate bounds"
                ));
            }
        }
        Ok(Self {
            positions: wire.positions,
        })
    }
}

impl FieldLayout {
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }
}

const STATE: Color32 = Color32::from_rgb(116, 171, 230);
const ACTION: Color32 = Color32::from_rgb(98, 199, 158);
const EVENT: Color32 = Color32::from_rgb(236, 181, 99);
const VALUE: Color32 = Color32::from_rgb(91, 203, 205);
const ERROR: Color32 = Color32::from_rgb(235, 125, 110);

pub(crate) fn describe(graph: &BehaviorGraph, reference: &FieldRef) -> (String, String, Color32) {
    match reference {
        FieldRef::Expression { root, path } => {
            super::field_formula::describe_expression(graph, root, path)
        }
        FieldRef::Predicate { transition, path } => {
            super::field_formula::describe_predicate(graph, *transition, path)
        }
        FieldRef::Graph => (
            "Controller".into(),
            format!("Start: {}", graph.initial),
            STATE,
        ),
        FieldRef::State(name) => {
            if let Some(binding) = graph
                .states
                .iter()
                .find(|s| s.name == *name)
                .and_then(|s| s.status.as_ref())
            {
                let label = match &binding.target {
                    super::status_model::StatusRef::Native(symbol) => match symbol.as_str() {
                        "FIGHTER_STATUS_KIND_SPECIAL_S" => "Side special".into(),
                        "FIGHTER_STATUS_KIND_SPECIAL_N" => "Neutral special".into(),
                        "FIGHTER_STATUS_KIND_SPECIAL_HI" => "Up special".into(),
                        "FIGHTER_STATUS_KIND_SPECIAL_LW" => "Down special".into(),
                        _ => symbol
                            .rsplit("_STATUS_KIND_")
                            .next()
                            .unwrap_or(symbol)
                            .replace('_', " ")
                            .to_ascii_lowercase(),
                    },
                    super::status_model::StatusRef::Custom { key, .. } => key.clone(),
                };
                let (ground, air) = super::status_recipes::motions(graph, name);
                // Owned is the authored case; only the game-logic policy needs saying.
                let title = if label == *name {
                    name.clone()
                } else {
                    format!("{name} · {label}")
                };
                let title = match binding.policy {
                    super::status_model::Policy::Owned => title,
                    super::status_model::Policy::Native => format!("{title} · game logic"),
                };
                (
                    title,
                    if air.is_empty() {
                        ground
                    } else {
                        format!("{ground} / {air}")
                    },
                    STATE,
                )
            } else {
                (
                    if graph.initial == *name {
                        "Entry state"
                    } else {
                        "State"
                    }
                    .into(),
                    name.clone(),
                    STATE,
                )
            }
        }
        FieldRef::Variable(name) => (
            "Variable".into(),
            graph.vars.iter().find(|var| var.name == *name).map_or_else(
                || name.clone(),
                |var| format!("{} · {}", name, var_kind_label(var.kind)),
            ),
            VALUE,
        ),
        FieldRef::EventField { event, field } => (
            "Event field".into(),
            format!(
                "{} · {}",
                event.chars().take(48).collect::<String>(),
                field.chars().take(48).collect::<String>()
            ),
            VALUE,
        ),
        FieldRef::Transition(index) => {
            // A rule card is titled by when it runs; the detail says what it checks and where it goes.
            let (title, detail) = graph.transitions.get(*index).map_or_else(
                || (format!("Rule {}", index + 1), String::new()),
                |transition| transition_parts(graph, *index, transition),
            );
            (
                title,
                if detail.is_empty() {
                    "always".into()
                } else {
                    detail
                },
                STATE,
            )
        }
        FieldRef::Action { transition, index } => graph
            .transitions
            .get(*transition)
            .and_then(|value| value.actions.get(*index))
            .map(|action| {
                let (title, detail) = match action {
                    Action::Status(_)
                    | Action::AddHeldOpponentDamage { .. }
                    | Action::AddDamage { .. }
                    | Action::CombatPolicy { .. }
                    | Action::ThrowHeldOpponent { .. }
                    | Action::ReleaseHeldOpponent
                    | Action::CancelIncomingHit
                    | Action::Movement(_)
                    | Action::Resource(_)
                    | Action::Item(_)
                    | Action::MeshVisibility { .. }
                    | Action::SetMotionRate { .. }
                    | Action::AddSpeed { .. }
                    | Action::ChooseHeroMenuSpell { .. } => super::graph_view::motion_text(action),
                    Action::SetNative { .. }
                    | Action::AddNative { .. }
                    | Action::SetNativeFlag { .. } => super::graph_view::native_write_text(action),
                    Action::Set { var, value } => (
                        if graph.vars.iter().any(|variable| {
                            variable.name == *var && variable.kind == VarType::Timer
                        }) {
                            "Start timer"
                        } else {
                            "Set value"
                        },
                        format!("{var} = {}", super::value_edit::expr_text(value)),
                    ),
                    Action::Add { var, value } => (
                        "Add to variable",
                        format!("{var} += {}", super::value_edit::expr_text(value)),
                    ),
                    Action::SetFlag { var, .. } => ("Set flag", var.clone()),
                    Action::Raise { var } => ("Raise flag", var.clone()),
                    Action::Clear { var } => ("Clear flag", var.clone()),
                    Action::Reset { var } => ("Reset value", var.clone()),
                    Action::EmitEvent { name } => ("Emit event", name.clone()),
                    Action::EmitEventArguments { name, arguments } => (
                        "Emit values · read-only",
                        format!("{name} · {} fields", arguments.len()),
                    ),
                    Action::EmitEventValues { name, arguments } => (
                        "Emit calculated values · read-only",
                        format!("{name} · {} fields", arguments.len()),
                    ),
                    Action::GenerateArticle { symbol } => (
                        "Spawn article",
                        symbol.rsplit('_').next().unwrap_or(symbol).to_string(),
                    ),
                    Action::RequestArticleStatus { symbol, status } => (
                        "Article status",
                        format!(
                            "{} · {}",
                            super::editor::article_label(symbol),
                            super::editor::article_status_label(status)
                        ),
                    ),
                    Action::RemoveNewestArticle { symbol } => (
                        "Remove newest article",
                        super::editor::article_label(symbol),
                    ),
                    Action::ArticleMovement { symbol, op } => {
                        (op.label(), super::editor::article_label(symbol))
                    }
                    Action::Object { symbol, op } => {
                        (op.label(), super::editor::article_label(symbol))
                    }
                    Action::RemoveArticle { symbol } => (
                        "Remove article",
                        symbol.rsplit('_').next().unwrap_or(symbol).to_string(),
                    ),
                    Action::RequestFighterStatus { symbol } => (
                        "Change status",
                        crate::gimmicks::field_canvas::status_name(symbol),
                    ),
                    Action::RequestWeaponStatus { symbol, .. } => (
                        "Weapon status",
                        symbol.rsplit('_').next().unwrap_or(symbol).to_string(),
                    ),
                };
                (title.into(), detail, ACTION)
            })
            .unwrap_or_else(|| ("Missing action".into(), String::new(), ERROR)),
        FieldRef::SourceBinding(source) => match source {
            SourceBindingRef::Hook(index) => (
                "Event source".into(),
                graph
                    .emission_plan
                    .bindings
                    .get(*index)
                    .map_or_else(String::new, |binding| {
                        super::graph_view::hook_label(binding.hook).into()
                    }),
                EVENT,
            ),
            SourceBindingRef::Event(index) => (
                "Event receiver".into(),
                graph
                    .emission_plan
                    .event_bindings
                    .get(*index)
                    .map_or_else(String::new, |binding| binding.name.clone()),
                EVENT,
            ),
            SourceBindingRef::Status(index) => (
                graph.emission_plan.status_triggers.get(*index).map_or_else(
                    || "Status source".into(),
                    |source| super::graph_view::hook_label(source.hook).into(),
                ),
                graph
                    .emission_plan
                    .status_triggers
                    .get(*index)
                    .map_or_else(String::new, |source| source.symbol.clone()),
                EVENT,
            ),
            SourceBindingRef::Resource(index) => (
                "Resource source".into(),
                graph
                    .emission_plan
                    .resource_triggers
                    .get(*index)
                    .map_or_else(String::new, |source| source.source.label().into()),
                EVENT,
            ),
        },
        _ => ("Unresolved reference".into(), reference.key(), ERROR),
    }
}

/// What runs a transition, when it passes, and where it goes:
/// `meter_gain · meter < 100`, `Every frame · in ATTACK_S4 → charged`.
#[cfg(test)]
fn transition_text(
    graph: &BehaviorGraph,
    index: usize,
    transition: &super::behavior::Transition,
) -> String {
    let (title, detail) = transition_parts(graph, index, transition);
    if detail.is_empty() {
        title
    } else if detail.starts_with('→') {
        format!("{title} {detail}")
    } else {
        format!("{title} · {detail}")
    }
}

/// When a rule runs (its hooks, events or status phase), and what it checks and where it goes.
fn transition_parts(
    graph: &BehaviorGraph,
    index: usize,
    transition: &super::behavior::Transition,
) -> (String, String) {
    let plan = &graph.emission_plan;
    let mut sources: Vec<String> = plan
        .bindings
        .iter()
        .filter(|binding| binding.transitions.contains(&index))
        .map(|binding| super::graph_view::hook_label(binding.hook).to_owned())
        .collect();
    sources.extend(
        plan.event_bindings
            .iter()
            .filter(|binding| binding.transitions.contains(&index))
            .map(|binding| binding.name.clone()),
    );
    if let Some(phase) = super::status_model::phase(
        graph
            .states
            .iter()
            .filter_map(|state| state.status.as_ref()),
        index,
    ) {
        use super::status_model::Phase;
        sources.push(
            match phase {
                Phase::Pre => "Status setup",
                Phase::Init => "Status init",
                Phase::MainEnter => "Status start",
                Phase::MainLoop => "Status frame",
                Phase::Exec => "Status exec",
                Phase::End => "Status end",
                Phase::Exit => "Status exit",
            }
            .to_owned(),
        );
    }
    let title = if sources.is_empty() {
        "Unbound rule".to_owned()
    } else {
        sources.join(", ")
    };
    let always = matches!(&transition.guard, super::behavior::Predicate::Compare { op: super::behavior::CmpOp::Eq, left: super::behavior::Expr::Const(a), right: super::behavior::Expr::Const(b) } if a == b);
    let mut detail = if always {
        String::new()
    } else {
        super::value_edit::predicate_text(&transition.guard)
    };
    if transition.from != transition.to {
        if !detail.is_empty() {
            detail.push(' ');
        }
        detail.push_str(&format!("→ {}", transition.to));
    }
    (title, detail)
}

pub(crate) fn status_name(symbol: &str) -> String {
    symbol
        .split_once("_STATUS_KIND_")
        .map_or(symbol, |(_, name)| name)
        .replace('_', " ")
        .to_lowercase()
}

/// The palette's names for variable kinds.
pub(crate) fn var_kind_label(kind: super::behavior::VarType) -> &'static str {
    match kind {
        super::behavior::VarType::Int => "Counter",
        super::behavior::VarType::Float => "Gauge",
        super::behavior::VarType::Flag => "Flag",
        super::behavior::VarType::Timer => "Timer",
    }
}

#[cfg(test)]
mod tests {
    use super::super::behavior::{
        CmpOp, ConstValue, Expr, Predicate, StateDef, Transition, VarDef,
    };
    use super::super::codegen::{EmissionPlan, HookBinding};
    use super::super::hooks::HookName;
    use super::*;

    fn fixture() -> BehaviorGraph {
        BehaviorGraph {
            custom_statuses: Default::default(),
            initial: "idle".into(),
            states: vec![
                StateDef {
                    status: None,
                    name: "idle".into(),
                },
                StateDef {
                    status: None,
                    name: "active".into(),
                },
            ],
            vars: ["original", "rerouted"]
                .into_iter()
                .map(|name| VarDef {
                    keep_across_form_switch: false,
                    name: name.into(),
                    kind: VarType::Int,
                    initial: ConstValue::Int(0),
                })
                .collect(),
            transitions: vec![Transition {
                from: "idle".into(),
                to: "idle".into(),
                on_frame: true,
                guard: Predicate::Compare {
                    op: CmpOp::Eq,
                    left: Expr::Const(ConstValue::Int(0)),
                    right: Expr::Const(ConstValue::Int(0)),
                },
                actions: vec![Action::Set {
                    var: "original".into(),
                    value: Expr::Const(ConstValue::Int(42)),
                }],
            }],
            emission_plan: EmissionPlan {
                bindings: vec![HookBinding {
                    hook: HookName::OnChargeTick,
                    article: None,
                    transitions: vec![0],
                }],
                ..Default::default()
            },
        }
    }

    #[test]
    fn layouts_roundtrip_independently_and_old_projects_receive_defaults() {
        let mut project = crate::mod_project::ModProjectFile::default();
        let fighter = project.fighters.entry("mario".into()).or_default();
        fighter.behavior = fixture();
        fighter
            .behavior_layout
            .positions
            .insert(FieldRef::State("idle".into()).key(), [17.0, 83.0]);
        let json = serde_json::to_string(&project).unwrap();
        let reloaded: crate::mod_project::ModProjectFile = serde_json::from_str(&json).unwrap();
        assert_eq!(reloaded.fighters["mario"].behavior, fixture());
        assert_eq!(
            reloaded.fighters["mario"].behavior_layout,
            project.fighters["mario"].behavior_layout
        );
        let mut legacy: serde_json::Value = serde_json::from_str(&json).unwrap();
        legacy["fighters"]["mario"]
            .as_object_mut()
            .unwrap()
            .remove("behavior_layout");
        let old: crate::mod_project::ModProjectFile = serde_json::from_value(legacy).unwrap();
        assert!(old.fighters["mario"].behavior_layout.is_empty());
        assert_eq!(old.fighters["mario"].behavior, fixture());
    }

    #[test]
    fn invalid_layouts_are_rejected() {
        assert!(
            serde_json::from_str::<FieldLayout>(r#"{"positions":{"wrong-key":[1,2]}}"#).is_err()
        );
        let invalid = format!(
            r#"{{"positions":{{"{}":[100001,0]}}}}"#,
            FieldRef::Graph.key()
        );
        assert!(serde_json::from_str::<FieldLayout>(&invalid).is_err());
    }

    #[test]
    fn owned_status_rules_name_their_phase_and_hide_the_always_guard() {
        let mut graph = fixture();
        graph.transitions.clear();
        graph.emission_plan = Default::default();
        let graph = super::super::graph_edit::add_state(&graph, "mario", "cape").unwrap();
        let graph = super::super::graph_edit::set_state_status(
            &graph,
            "mario",
            "cape",
            Some("FIGHTER_STATUS_KIND_SPECIAL_S"),
        )
        .unwrap();
        let texts: Vec<_> = graph
            .transitions
            .iter()
            .enumerate()
            .map(|(i, t)| transition_text(&graph, i, t))
            .collect();
        assert_eq!(texts[0], "Status setup");
        assert_eq!(texts[1], "Status start");
        assert!(
            texts[2].starts_with("Status frame · motion ended"),
            "{}",
            texts[2]
        );
        assert!(
            texts
                .iter()
                .all(|text| !text.contains("Unbound") && !text.contains("1 == 1")),
            "{texts:?}"
        );
    }
}
