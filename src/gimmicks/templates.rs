//! Portable behavior templates and reusable internal-state controllers.
//!
//! Templates are serialized behavior graphs, not native fighter gimmicks.
//! Their hook bindings describe generic runtime entry points. Any fighter-
//! specific action remains subject to the pinned registry and adapter gates
//! when a template is instantiated.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::behavior::{
    Action, BehaviorGraph, CmpOp, ConstValue, Expr, Predicate, StateDef, Transition, VarDef,
    VarType,
};
use super::codegen::{EmissionPlan, HookBinding, ResourceSource, ResourceTrigger};
use super::hooks::HookName;

const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
const MAX_NAME_CHARS: usize = 128;
const MAX_DESCRIPTION_CHARS: usize = 4096;

/// Ready-to-use internal-state controllers. These do not claim to implement
/// any fighter's native resource or mechanics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// Count observed KO events for this fighter object.
    KoTally,
    /// Cycle an internal 60-frame timer.
    Timer,
    /// Advance an internal gauge to 100, then reset it.
    Charge,
    /// A 0–100 meter that moves fill with `meter_gain` and empty with `meter_spend`.
    Meter,
    /// Three stances; each `stance_next` event moves to the next one.
    Stance,
    /// Three charges; `ammo_use` spends one, and they recharge one at a time.
    Ammo,
    /// A 0–100 rage gauge filled by the damage this fighter deals.
    Rage,
    /// Hits landed within 90 frames of each other; `combo_spend` resets it.
    Combo,
}

impl Preset {
    pub const ALL: [Self; 8] = [
        Self::KoTally,
        Self::Timer,
        Self::Charge,
        Self::Meter,
        Self::Stance,
        Self::Ammo,
        Self::Rage,
        Self::Combo,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::KoTally => "KO tally",
            Self::Timer => "Repeating 60-frame timer",
            Self::Charge => "100-point charge gauge",
            Self::Meter => "Meter filled by moves",
            Self::Stance => "Cycling stance",
            Self::Ammo => "Ammo with recharge",
            Self::Rage => "Rage from damage dealt",
            Self::Combo => "Hit combo counter",
        }
    }

    /// Short name new controllers made from this preset are numbered after.
    pub fn stem(self) -> &'static str {
        match self {
            Self::KoTally => "ko_tally",
            Self::Timer => "timer",
            Self::Charge => "charge",
            Self::Meter => "meter",
            Self::Stance => "stance",
            Self::Ammo => "ammo",
            Self::Rage => "rage",
            Self::Combo => "combo",
        }
    }

    /// Build a portable generic controller with verified hook bindings.
    pub fn graph(self) -> BehaviorGraph {
        match self {
            Self::KoTally => BehaviorGraph {
                costumes: Vec::new(),
                custom_statuses: Default::default(),
                initial: "tracking".into(),
                states: vec![StateDef {
                    status: None,
                    name: "tracking".into(),
                }],
                vars: vec![VarDef {
                    keep_across_form_switch: false,
                    name: "stock_losses".into(),
                    kind: VarType::Int,
                    initial: ConstValue::Int(0),
                }],
                transitions: vec![Transition {
                    from: "tracking".into(),
                    to: "tracking".into(),
                    guard: Predicate::Compare {
                        op: CmpOp::Gte,
                        left: Expr::Var("stock_losses".into()),
                        right: Expr::Const(ConstValue::Int(0)),
                    },
                    on_frame: true,
                    actions: vec![Action::Add {
                        var: "stock_losses".into(),
                        value: Expr::Const(ConstValue::Int(1)),
                    }],
                }],
                emission_plan: EmissionPlan {
                    bindings: vec![HookBinding {
                        hook: HookName::OnKo,
                        article: None,
                        transitions: vec![0],
                    }],
                    ..EmissionPlan::default()
                },
            },
            Self::Timer => BehaviorGraph {
                costumes: Vec::new(),
                custom_statuses: Default::default(),
                initial: "waiting".into(),
                states: vec![StateDef {
                    status: None,
                    name: "waiting".into(),
                }],
                vars: vec![
                    VarDef {
                        keep_across_form_switch: false,
                        name: "remaining".into(),
                        kind: VarType::Timer,
                        initial: ConstValue::Int(60),
                    },
                    VarDef {
                        keep_across_form_switch: false,
                        name: "elapsed_events".into(),
                        kind: VarType::Int,
                        initial: ConstValue::Int(0),
                    },
                    VarDef {
                        keep_across_form_switch: false,
                        name: "ready".into(),
                        kind: VarType::Flag,
                        initial: ConstValue::Flag(false),
                    },
                ],
                transitions: vec![
                    Transition {
                        from: "waiting".into(),
                        to: "waiting".into(),
                        guard: Predicate::Flag("ready".into()),
                        on_frame: true,
                        actions: vec![Action::Clear {
                            var: "ready".into(),
                        }],
                    },
                    Transition {
                        from: "waiting".into(),
                        to: "waiting".into(),
                        guard: Predicate::Expired("remaining".into()),
                        on_frame: true,
                        actions: vec![
                            Action::Add {
                                var: "elapsed_events".into(),
                                value: Expr::Const(ConstValue::Int(1)),
                            },
                            Action::Raise {
                                var: "ready".into(),
                            },
                            Action::Set {
                                var: "remaining".into(),
                                value: Expr::Const(ConstValue::Int(60)),
                            },
                        ],
                    },
                    Transition {
                        from: "waiting".into(),
                        to: "waiting".into(),
                        guard: Predicate::Compare {
                            op: CmpOp::Gte,
                            left: Expr::Var("elapsed_events".into()),
                            right: Expr::Const(ConstValue::Int(0)),
                        },
                        on_frame: true,
                        actions: vec![
                            Action::Set {
                                var: "elapsed_events".into(),
                                value: Expr::Const(ConstValue::Int(0)),
                            },
                            Action::Clear {
                                var: "ready".into(),
                            },
                            Action::Set {
                                var: "remaining".into(),
                                value: Expr::Const(ConstValue::Int(60)),
                            },
                        ],
                    },
                ],
                emission_plan: EmissionPlan {
                    bindings: vec![
                        HookBinding {
                            hook: HookName::OnChargeTick,
                            article: None,
                            transitions: vec![0],
                        },
                        HookBinding {
                            hook: HookName::OnTimeout,
                            article: None,
                            transitions: vec![1],
                        },
                        HookBinding {
                            hook: HookName::OnStockStart,
                            article: None,
                            transitions: vec![2],
                        },
                    ],
                    ..EmissionPlan::default()
                },
            },
            Self::Meter => meter(),
            Self::Stance => stance(),
            Self::Ammo => ammo(),
            Self::Rage => rage(),
            Self::Combo => combo(),
            Self::Charge => BehaviorGraph {
                costumes: Vec::new(),
                custom_statuses: Default::default(),
                initial: "charging".into(),
                states: vec![StateDef {
                    status: None,
                    name: "charging".into(),
                }],
                vars: vec![
                    VarDef {
                        keep_across_form_switch: false,
                        name: "gauge".into(),
                        kind: VarType::Float,
                        initial: ConstValue::Float(0.0),
                    },
                    VarDef {
                        keep_across_form_switch: false,
                        name: "ready".into(),
                        kind: VarType::Flag,
                        initial: ConstValue::Flag(false),
                    },
                ],
                transitions: vec![
                    Transition {
                        from: "charging".into(),
                        to: "charging".into(),
                        guard: Predicate::Flag("ready".into()),
                        on_frame: true,
                        actions: vec![Action::Clear {
                            var: "ready".into(),
                        }],
                    },
                    Transition {
                        from: "charging".into(),
                        to: "charging".into(),
                        guard: Predicate::Compare {
                            op: CmpOp::Gte,
                            left: Expr::Var("gauge".into()),
                            right: Expr::Const(ConstValue::Float(100.0)),
                        },
                        on_frame: true,
                        actions: vec![
                            Action::Reset {
                                var: "gauge".into(),
                            },
                            Action::Raise {
                                var: "ready".into(),
                            },
                        ],
                    },
                    Transition {
                        from: "charging".into(),
                        to: "charging".into(),
                        guard: Predicate::And(
                            Box::new(Predicate::Compare {
                                op: CmpOp::Lt,
                                left: Expr::Var("gauge".into()),
                                right: Expr::Const(ConstValue::Float(100.0)),
                            }),
                            Box::new(Predicate::Not(Box::new(Predicate::Flag("ready".into())))),
                        ),
                        on_frame: true,
                        actions: vec![Action::Add {
                            var: "gauge".into(),
                            value: Expr::Const(ConstValue::Float(1.0)),
                        }],
                    },
                ],
                emission_plan: EmissionPlan {
                    bindings: vec![HookBinding {
                        hook: HookName::OnChargeTick,
                        article: None,
                        transitions: vec![0, 1, 2],
                    }],
                    ..EmissionPlan::default()
                },
            },
        }
    }
}

/// A versioned portable document containing a constrained behavior graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BehaviorTemplate {
    pub schema_version: u32,
    pub name: String,
    pub description: String,
    pub source_fighter: Option<String>,
    pub graph: BehaviorGraph,
}

impl BehaviorTemplate {
    /// Validate a graph and attach optional donor provenance.
    pub fn from_graph(
        name: &str,
        source_fighter: Option<&str>,
        graph: BehaviorGraph,
    ) -> Result<Self, String> {
        let template = Self {
            schema_version: 1,
            name: name.to_owned(),
            description: String::new(),
            source_fighter: source_fighter.map(str::to_owned),
            graph,
        };
        template.validate()?;
        Ok(template)
    }

    /// Parse a bounded, versioned template document and validate every gate.
    pub fn decode(json: &str) -> Result<Self, String> {
        if json.len() > MAX_DOCUMENT_BYTES {
            return Err(format!(
                "template document exceeds {MAX_DOCUMENT_BYTES} bytes"
            ));
        }
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(|error| format!("invalid template JSON: {error}"))?;
        reject_unknown_graph_fields(&value)?;
        let template: Self = serde_json::from_value(value)
            .map_err(|error| format!("invalid template document: {error}"))?;
        template.validate()?;
        Ok(template)
    }

    /// Serialize a validated document in a stable, readable form.
    pub fn encode(&self) -> Result<String, String> {
        self.validate()?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|error| format!("could not encode behavior template: {error}"))?;
        if json.len() > MAX_DOCUMENT_BYTES {
            return Err(format!(
                "template document exceeds {MAX_DOCUMENT_BYTES} bytes"
            ));
        }
        Ok(json)
    }

    /// Clone the graph for a target fighter only after exact registry,
    /// hook-plan, verifier, and adapter checks pass.
    pub fn instantiate(&self, fighter: &str) -> Result<BehaviorGraph, String> {
        self.validate()?;
        validate_for_target(&self.graph, fighter)?;
        Ok(self.graph.clone())
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err(format!(
                "unsupported template schema version {}",
                self.schema_version
            ));
        }
        validate_text("name", &self.name, MAX_NAME_CHARS, false)?;
        validate_text(
            "description",
            &self.description,
            MAX_DESCRIPTION_CHARS,
            true,
        )?;
        if let Some(fighter) = &self.source_fighter {
            validate_text("source_fighter", fighter, MAX_NAME_CHARS, false)?;
            if super::registry::fighter_kind_symbol(fighter).is_none() {
                return Err(format!("unknown source fighter {fighter:?}"));
            }
            validate_for_target(&self.graph, fighter)?;
        }
        if let Some(reason) = portability_violation(&self.graph) {
            if self.source_fighter.is_none() {
                return Err(format!("portable template {reason}"));
            }
        }
        validate_generic_graph(
            &self.graph,
            self.source_fighter.as_deref().unwrap_or("template"),
        )
    }
}

/// Copy a graph while retaining fail-closed target validation.
pub fn copy_for_fighter(
    graph: &BehaviorGraph,
    source: &str,
    target: &str,
) -> Result<BehaviorGraph, String> {
    BehaviorTemplate::from_graph("Copied behavior", Some(source), graph.clone())?
        .instantiate(target)
}

fn validate_text(
    field: &str,
    text: &str,
    max_chars: usize,
    allow_empty: bool,
) -> Result<(), String> {
    if !allow_empty && text.trim().is_empty() {
        return Err(format!("template {field} must not be empty"));
    }
    if text.chars().count() > max_chars {
        return Err(format!("template {field} exceeds {max_chars} characters"));
    }
    Ok(())
}

/// `owner` is the fighter a graph is bound to, or a generic name for a portable template.
fn validate_generic_graph(graph: &BehaviorGraph, owner: &str) -> Result<(), String> {
    let errors = super::behavior::validate(graph);
    if !errors.is_empty() {
        return Err(errors
            .into_iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("; "));
    }
    let plan_errors = super::codegen::check_plan(graph, &graph.emission_plan);
    if !plan_errors.is_empty() {
        return Err(plan_errors
            .into_iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("; "));
    }
    // Donor validation runs validate_for_target first, where these exact
    // status/resource proofs are checked against the source fighter.
    if graph.emission_plan.status_triggers.is_empty()
        && graph.emission_plan.resource_triggers.is_empty()
    {
        let report = super::verifier::verify_behavior(owner, graph, &graph.emission_plan);
        let blockers: Vec<_> = report.blockers().collect();
        if let Some(finding) = blockers.first() {
            return Err(format!("{}: {}", finding.subject, finding.message));
        }
    }
    Ok(())
}

fn validate_for_target(graph: &BehaviorGraph, fighter: &str) -> Result<(), String> {
    if super::registry::fighter_kind_symbol(fighter).is_none() {
        return Err(format!(
            "fighter {fighter:?} has no exact pinned fighter-kind entry"
        ));
    }
    let errors = super::behavior::validate_for_fighter(fighter, graph);
    if !errors.is_empty() {
        return Err(errors
            .into_iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("; "));
    }
    let status_errors = super::codegen::check_status_triggers(fighter, &graph.emission_plan);
    if !status_errors.is_empty() {
        return Err(status_errors
            .into_iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("; "));
    }
    let report = super::verifier::verify_behavior(fighter, graph, &graph.emission_plan);
    let blockers: Vec<_> = report.blockers().collect();
    if let Some(finding) = blockers.first() {
        return Err(format!("{}: {}", finding.subject, finding.message));
    }
    super::adapter::render(&BTreeMap::from([(fighter.to_owned(), graph.clone())])).map_err(
        |errors| {
            errors
                .into_iter()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        },
    )?;
    Ok(())
}

fn portability_violation(graph: &BehaviorGraph) -> Option<String> {
    if !graph.emission_plan.status_triggers.is_empty() {
        return Some("contains fighter-owned status proofs".into());
    }
    if !graph.emission_plan.resource_triggers.is_empty() {
        return Some("contains fighter-specific native resource sources".into());
    }
    for (index, transition) in graph.transitions.iter().enumerate() {
        for (action_index, action) in transition.actions.iter().enumerate() {
            if matches!(
                action,
                Action::GenerateArticle { .. }
                    | Action::RemoveArticle { .. }
                    | Action::RemoveNewestArticle { .. }
                    | Action::ArticleMovement { .. }
                    | Action::RequestArticleStatus { .. }
                    | Action::RequestFighterStatus { .. }
                    | Action::RequestWeaponStatus { .. }
            ) {
                return Some(format!("contains fighter-specific engine action at transition {index}, action {action_index}"));
            }
        }
    }
    None
}

/// Reject unknown keys inside graph nodes as well as at the template root.
/// The graph IR predates strict nested serde and intentionally remains
/// backwards compatible for project files; imported template documents are
/// stricter so misspellings cannot silently disappear.
fn reject_unknown_graph_fields(value: &serde_json::Value) -> Result<(), String> {
    let Some(root) = value.as_object() else {
        return Ok(());
    };
    let Some(graph) = root.get("graph").and_then(serde_json::Value::as_object) else {
        return Ok(());
    };
    reject_keys(
        graph,
        &["initial", "states", "vars", "transitions", "emission_plan"],
        "graph",
    )?;
    for (field, allowed) in [
        ("states", &["name"][..]),
        (
            "vars",
            &["name", "kind", "initial", "keep_across_form_switch"][..],
        ),
        (
            "transitions",
            &["from", "to", "guard", "on_frame", "actions"][..],
        ),
    ] {
        if let Some(items) = graph.get(field).and_then(serde_json::Value::as_array) {
            for (index, item) in items.iter().enumerate() {
                if let Some(object) = item.as_object() {
                    reject_keys(object, allowed, &format!("graph.{field}[{index}]"))?;
                    if field == "vars" {
                        if let Some(initial) = object.get("initial") {
                            reject_enum_payload(
                                initial,
                                &["Float", "Int", "Flag"],
                                &format!("graph.vars[{index}].initial"),
                            )?;
                        }
                    }
                }
                if field == "transitions" {
                    if let Some(guard) = item.get("guard") {
                        reject_predicate(guard, &format!("graph.transitions[{index}].guard"))?;
                    }
                    if let Some(actions) = item.get("actions").and_then(serde_json::Value::as_array)
                    {
                        for (action_index, action) in actions.iter().enumerate() {
                            reject_action_keys(
                                action,
                                &format!("graph.transitions[{index}].actions[{action_index}]"),
                            )?;
                        }
                    }
                }
            }
        }
    }
    if let Some(plan) = graph
        .get("emission_plan")
        .and_then(serde_json::Value::as_object)
    {
        reject_keys(
            plan,
            &["bindings", "status_triggers", "resource_triggers"],
            "graph.emission_plan",
        )?;
        if let Some(bindings) = plan.get("bindings").and_then(serde_json::Value::as_array) {
            for (i, binding) in bindings.iter().enumerate() {
                if let Some(object) = binding.as_object() {
                    reject_keys(
                        object,
                        &["hook", "transitions"],
                        &format!("graph.emission_plan.bindings[{i}]"),
                    )?;
                }
            }
        }
        if let Some(triggers) = plan
            .get("status_triggers")
            .and_then(serde_json::Value::as_array)
        {
            for (i, trigger) in triggers.iter().enumerate() {
                if let Some(object) = trigger.as_object() {
                    reject_keys(
                        object,
                        &["hook", "symbol", "evidence", "runtime_validated"],
                        &format!("graph.emission_plan.status_triggers[{i}]"),
                    )?;
                }
            }
        }
        if let Some(triggers) = plan
            .get("resource_triggers")
            .and_then(serde_json::Value::as_array)
        {
            for (i, trigger) in triggers.iter().enumerate() {
                if let Some(object) = trigger.as_object() {
                    reject_keys(
                        object,
                        &["hook", "source"],
                        &format!("graph.emission_plan.resource_triggers[{i}]"),
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn reject_action_keys(value: &serde_json::Value, path: &str) -> Result<(), String> {
    let Some((variant, payload)) = sole_variant(value) else {
        return Ok(());
    };
    let (allowed, expression_fields): (&[&str], &[&str]) = match variant {
        "Set" | "Add" => (&["var", "value"], &["value"]),
        "Raise" | "Clear" | "Reset" => (&["var"], &[]),
        "GenerateArticle" | "RemoveArticle" | "RequestFighterStatus" => (&["symbol"], &[]),
        "RequestWeaponStatus" => (&["target", "symbol"], &[]),
        _ => return Ok(()),
    };
    if let Some(fields) = payload.as_object() {
        reject_keys(fields, allowed, path)?;
        for field in expression_fields {
            if let Some(expression) = fields.get(*field) {
                reject_expr(expression, &format!("{path}.{field}"))?;
            }
        }
        if let Some(target) = fields.get("target").and_then(serde_json::Value::as_object) {
            reject_keys(target, &["battle_object_id"], &format!("{path}.target"))?;
        }
    }
    Ok(())
}

fn sole_variant(value: &serde_json::Value) -> Option<(&str, &serde_json::Value)> {
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    let (variant, payload) = object.iter().next()?;
    Some((variant.as_str(), payload))
}

fn reject_enum_payload(
    value: &serde_json::Value,
    variants: &[&str],
    path: &str,
) -> Result<(), String> {
    if let Some((variant, _)) = sole_variant(value) {
        if !variants.contains(&variant) {
            return Err(format!("unknown variant {variant:?} at {path}"));
        }
    }
    Ok(())
}

fn reject_expr(value: &serde_json::Value, path: &str) -> Result<(), String> {
    let Some((variant, payload)) = sole_variant(value) else {
        return Ok(());
    };
    match variant {
        "Const" => reject_enum_payload(payload, &["Float", "Int", "Flag"], path),
        "Var" | "PendingDamage" | "TimerRemaining" | "StatusFrame" | "MotionFrame" | "StickX"
        | "StickY" | "RandomInt" | "RandomFloat" => Ok(()),
        "Add" | "Sub" | "Mul" => {
            if let Some(pair) = payload.as_array() {
                for (index, expression) in pair.iter().enumerate() {
                    reject_expr(expression, &format!("{path}.{variant}[{index}]"))?;
                }
            }
            Ok(())
        }
        _ => Err(format!("unknown expression variant {variant:?} at {path}")),
    }
}

fn reject_predicate(value: &serde_json::Value, path: &str) -> Result<(), String> {
    let Some((variant, payload)) = sole_variant(value) else {
        return Ok(());
    };
    match variant {
        "Flag" | "Expired" => Ok(()),
        "Compare" => {
            if let Some(fields) = payload.as_object() {
                reject_keys(fields, &["op", "left", "right"], path)?;
                for key in ["left", "right"] {
                    if let Some(expression) = fields.get(key) {
                        reject_expr(expression, &format!("{path}.{key}"))?;
                    }
                }
            }
            Ok(())
        }
        "Not" => reject_predicate(payload, path),
        "And" | "Or" => {
            if let Some(pair) = payload.as_array() {
                for (index, predicate) in pair.iter().enumerate() {
                    reject_predicate(predicate, &format!("{path}.{variant}[{index}]"))?;
                }
            }
            Ok(())
        }
        _ => Err(format!("unknown predicate variant {variant:?} at {path}")),
    }
}

fn reject_keys(
    object: &serde_json::Map<String, serde_json::Value>,
    allowed: &[&str],
    path: &str,
) -> Result<(), String> {
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("unknown field {key:?} at {path}"));
    }
    Ok(())
}

// Event-driven controllers. Every passing transition bound to an event runs in list order and
// sees the values left by the ones before it, so each list is ordered to be correct under that.

pub(super) fn always() -> Predicate {
    Predicate::Compare {
        op: CmpOp::Eq,
        left: Expr::Const(ConstValue::Int(0)),
        right: Expr::Const(ConstValue::Int(0)),
    }
}

fn compare(op: CmpOp, var: &str, value: ConstValue) -> Predicate {
    Predicate::Compare {
        op,
        left: Expr::Var(var.into()),
        right: Expr::Const(value),
    }
}

fn set(var: &str, value: ConstValue) -> Action {
    Action::Set {
        var: var.into(),
        value: Expr::Const(value),
    }
}

/// One-state controller: `vars`, guarded actions, and which transitions each event or hook runs.
fn single_state(
    vars: Vec<VarDef>,
    transitions: Vec<(Predicate, Vec<Action>)>,
    events: &[(&str, &[usize])],
    hooks: &[(HookName, &[usize])],
) -> BehaviorGraph {
    BehaviorGraph {
        costumes: Vec::new(),
        custom_statuses: Default::default(),
        initial: "active".into(),
        states: vec![StateDef {
            status: None,
            name: "active".into(),
        }],
        vars,
        transitions: transitions
            .into_iter()
            .map(|(guard, actions)| Transition {
                from: "active".into(),
                to: "active".into(),
                guard,
                on_frame: true,
                actions,
            })
            .collect(),
        emission_plan: EmissionPlan {
            bindings: hooks
                .iter()
                .map(|(hook, transitions)| HookBinding {
                    hook: *hook,
                    article: None,
                    transitions: transitions.to_vec(),
                })
                .collect(),
            event_bindings: events
                .iter()
                .map(|(name, transitions)| super::codegen::EventBinding {
                    name: (*name).into(),
                    transitions: transitions.to_vec(),
                    fields: Vec::new(),
                })
                .collect(),
            ..EmissionPlan::default()
        },
    }
}

fn var(name: &str, kind: VarType, initial: ConstValue) -> VarDef {
    VarDef {
        keep_across_form_switch: false,
        name: name.into(),
        kind,
        initial,
    }
}

fn meter() -> BehaviorGraph {
    use ConstValue::Float;
    single_state(
        vec![var("meter", VarType::Float, Float(0.0))],
        vec![
            (
                compare(CmpOp::Lt, "meter", Float(100.0)),
                vec![Action::Add {
                    var: "meter".into(),
                    value: Expr::Const(Float(10.0)),
                }],
            ),
            (
                compare(CmpOp::Gt, "meter", Float(100.0)),
                vec![set("meter", Float(100.0))],
            ),
            (
                compare(CmpOp::Gte, "meter", Float(100.0)),
                vec![set("meter", Float(0.0))],
            ),
            (always(), vec![set("meter", Float(0.0))]),
        ],
        &[("meter_gain", &[0, 1]), ("meter_spend", &[2])],
        &[(HookName::OnStockStart, &[3])],
    )
}

fn stance() -> BehaviorGraph {
    use ConstValue::{Flag, Int};
    let step = |from: i32, to: i32| {
        (
            Predicate::And(
                Box::new(compare(CmpOp::Eq, "stance", Int(from))),
                Box::new(Predicate::Not(Box::new(Predicate::Flag("switched".into())))),
            ),
            vec![
                set("stance", Int(to)),
                Action::Raise {
                    var: "switched".into(),
                },
            ],
        )
    };
    single_state(
        vec![
            var("stance", VarType::Int, Int(0)),
            var("switched", VarType::Flag, Flag(false)),
        ],
        vec![
            // `switched` stops the next step from chaining within the same event.
            (
                always(),
                vec![Action::Clear {
                    var: "switched".into(),
                }],
            ),
            step(0, 1),
            step(1, 2),
            step(2, 0),
            (always(), vec![set("stance", Int(0))]),
        ],
        &[("stance_next", &[0, 1, 2, 3])],
        &[(HookName::OnStockStart, &[4])],
    )
}

fn ammo() -> BehaviorGraph {
    use ConstValue::Int;
    const MAX: i32 = 3;
    const RECHARGE: i32 = 120;
    single_state(
        vec![
            var("ammo", VarType::Int, Int(MAX)),
            var("recharge", VarType::Timer, Int(0)),
        ],
        vec![
            (
                compare(CmpOp::Gt, "ammo", Int(0)),
                vec![
                    Action::Add {
                        var: "ammo".into(),
                        value: Expr::Const(Int(-1)),
                    },
                    set("recharge", Int(RECHARGE)),
                ],
            ),
            (
                Predicate::And(
                    Box::new(Predicate::Expired("recharge".into())),
                    Box::new(compare(CmpOp::Lt, "ammo", Int(MAX))),
                ),
                vec![Action::Add {
                    var: "ammo".into(),
                    value: Expr::Const(Int(1)),
                }],
            ),
            // Still short after that charge: start the next one.
            (
                Predicate::And(
                    Box::new(Predicate::Expired("recharge".into())),
                    Box::new(compare(CmpOp::Lt, "ammo", Int(MAX))),
                ),
                vec![set("recharge", Int(RECHARGE))],
            ),
            (
                always(),
                vec![set("ammo", Int(MAX)), set("recharge", Int(0))],
            ),
        ],
        &[("ammo_use", &[0])],
        &[
            (HookName::OnTimeout, &[1, 2]),
            (HookName::OnStockStart, &[3]),
        ],
    )
}

fn rage() -> BehaviorGraph {
    use ConstValue::Float;
    single_state(
        vec![var("rage", VarType::Float, Float(0.0))],
        vec![
            (
                always(),
                vec![Action::Add {
                    var: "rage".into(),
                    value: Expr::PendingDamage,
                }],
            ),
            (
                compare(CmpOp::Gt, "rage", Float(100.0)),
                vec![set("rage", Float(100.0))],
            ),
            (always(), vec![set("rage", Float(0.0))]),
            (always(), vec![set("rage", Float(0.0))]),
        ],
        &[("rage_spend", &[2])],
        &[
            (HookName::OnDealDamage, &[0, 1]),
            (HookName::OnStockStart, &[3]),
        ],
    )
}

fn combo() -> BehaviorGraph {
    use ConstValue::Int;
    const WINDOW: i32 = 90;
    single_state(
        vec![
            var("hits", VarType::Int, Int(0)),
            var("window", VarType::Timer, Int(0)),
        ],
        vec![
            (
                always(),
                vec![
                    Action::Add {
                        var: "hits".into(),
                        value: Expr::Const(Int(1)),
                    },
                    set("window", Int(WINDOW)),
                ],
            ),
            (
                Predicate::Expired("window".into()),
                vec![set("hits", Int(0))],
            ),
            (always(), vec![set("hits", Int(0))]),
            (always(), vec![set("hits", Int(0)), set("window", Int(0))]),
        ],
        &[("combo_spend", &[2])],
        &[
            (HookName::OnDealDamage, &[0]),
            (HookName::OnTimeout, &[1]),
            (HookName::OnStockStart, &[3]),
        ],
    )
}

/// Cloud's Limit gauge: count grounded Blade Beam spends and idle Limit expiries.
fn limit_spend_starter() -> BehaviorGraph {
    BehaviorGraph {
        costumes: Vec::new(),
        custom_statuses: Default::default(),
        initial: "watching".into(),
        states: vec![StateDef {
            status: None,
            name: "watching".into(),
        }],
        vars: vec![VarDef {
            keep_across_form_switch: false,
            name: "spends".into(),
            kind: VarType::Int,
            initial: ConstValue::Int(0),
        }],
        transitions: vec![Transition {
            from: "watching".into(),
            to: "watching".into(),
            guard: Predicate::Compare {
                op: CmpOp::Gte,
                left: Expr::Var("spends".into()),
                right: Expr::Const(ConstValue::Int(0)),
            },
            on_frame: true,
            actions: vec![Action::Add {
                var: "spends".into(),
                value: Expr::Const(ConstValue::Int(1)),
            }],
        }],
        emission_plan: EmissionPlan {
            event_bindings: Vec::new(),
            bindings: vec![HookBinding {
                hook: HookName::OnSpend,
                article: None,
                transitions: vec![0],
            }],
            status_triggers: Vec::new(),
            resource_triggers: vec![ResourceTrigger {
                hook: HookName::OnSpend,
                source: ResourceSource::CloudLimitGroundedNeutral,
            }],
        },
    }
}

pub(crate) fn limit_event_starter() -> BehaviorGraph {
    let mut graph = limit_spend_starter();
    graph.vars.push(VarDef {
        keep_across_form_switch: false,
        name: "expiries".into(),
        kind: VarType::Int,
        initial: ConstValue::Int(0),
    });
    graph.transitions.push(Transition {
        from: "watching".into(),
        to: "watching".into(),
        on_frame: true,
        guard: Predicate::Compare {
            op: CmpOp::Gte,
            left: Expr::Var("expiries".into()),
            right: Expr::Const(ConstValue::Int(0)),
        },
        actions: vec![Action::Add {
            var: "expiries".into(),
            value: Expr::Const(ConstValue::Int(1)),
        }],
    });
    graph.emission_plan.bindings.push(HookBinding {
        hook: HookName::OnTimeout,
        article: None,
        transitions: vec![1],
    });
    graph.emission_plan.resource_triggers.push(ResourceTrigger {
        hook: HookName::OnTimeout,
        source: ResourceSource::CloudLimitIdleExpiry,
    });
    graph
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_verifies_and_exports_for_every_registered_fighter() {
        for preset in Preset::ALL {
            let graph = preset.graph();
            validate_generic_graph(&graph, "template").unwrap();
            for fighter in super::super::registry::FIGHTER_KINDS {
                validate_for_target(&graph, fighter.fighter).unwrap_or_else(|error| {
                    panic!("{} for {}: {error}", preset.label(), fighter.fighter)
                });
            }
        }
    }

    #[test]
    fn event_presets_step_once_per_event_in_list_order() {
        use super::super::live_model::Value;
        use super::super::native_inputs::NativeSnapshot;
        let none = NativeSnapshot::empty();
        let run = |preset: Preset| {
            let program =
                super::super::live_protocol::program_from_graph("mario", &preset.graph()).unwrap();
            let state = program.initial_state();
            (program, state)
        };
        let (stance, mut state) = run(Preset::Stance);
        for expected in [1, 2, 0, 1] {
            stance
                .step_event_with_snapshot(&mut state, "stance_next", &none)
                .unwrap();
            assert_eq!(state.values["stance"], Value::Int(expected));
        }
        let (meter, mut state) = run(Preset::Meter);
        for _ in 0..12 {
            meter
                .step_event_with_snapshot(&mut state, "meter_gain", &none)
                .unwrap();
        }
        assert_eq!(state.values["meter"], Value::Float(100.0));
        meter
            .step_event_with_snapshot(&mut state, "meter_spend", &none)
            .unwrap();
        assert_eq!(state.values["meter"], Value::Float(0.0));
        let (ammo, mut state) = run(Preset::Ammo);
        for _ in 0..4 {
            ammo.step_event_with_snapshot(&mut state, "ammo_use", &none)
                .unwrap();
        }
        assert_eq!(state.values["ammo"], Value::Int(0));
        for expected in [1, 2, 3] {
            for _ in 0..120 {
                ammo.step_frame(&mut state).unwrap();
            }
            assert_eq!(state.values["ammo"], Value::Int(expected));
        }
        let hit = |pending| {
            super::super::native_inputs::DamageInput::new(
                super::super::native_inputs::DamageDirection::Deal,
                pending,
            )
            .unwrap()
        };
        let (rage, mut state) = run(Preset::Rage);
        for _ in 0..3 {
            rage.step_damage_with_snapshot(&mut state, hit(40.0), &none)
                .unwrap();
        }
        assert_eq!(state.values["rage"], Value::Float(100.0));
        let (combo, mut state) = run(Preset::Combo);
        combo
            .step_damage_with_snapshot(&mut state, hit(5.0), &none)
            .unwrap();
        combo
            .step_damage_with_snapshot(&mut state, hit(5.0), &none)
            .unwrap();
        assert_eq!(state.values["hits"], Value::Int(2));
        for _ in 0..91 {
            combo.step_frame(&mut state).unwrap();
        }
        assert_eq!(state.values["hits"], Value::Int(0));
    }

    #[test]
    fn template_round_trip_and_outer_unknown_fields_are_strict() {
        let template =
            BehaviorTemplate::from_graph("Charge", None, Preset::Charge.graph()).unwrap();
        let json = template.encode().unwrap();
        assert_eq!(BehaviorTemplate::decode(&json).unwrap(), template);
        let unknown = json.replace(
            "\"schema_version\": 1",
            "\"schema_version\": 1,\n  \"unexpected\": true",
        );
        assert!(BehaviorTemplate::decode(&unknown)
            .unwrap_err()
            .contains("unknown field"));
        let unknown_graph = json.replace(
            "\"initial\": \"charging\"",
            "\"initial\": \"charging\", \"extra\": 1",
        );
        assert!(BehaviorTemplate::decode(&unknown_graph)
            .unwrap_err()
            .contains("unknown field"));
        let unknown_guard = json.replace("\"op\": \"Lt\"", "\"op\": \"Lt\", \"mistyped\": true");
        assert!(BehaviorTemplate::decode(&unknown_guard)
            .unwrap_err()
            .contains("unknown field"));
    }

    #[test]
    fn malformed_schema_graph_and_text_bounds_fail_closed() {
        let template =
            BehaviorTemplate::from_graph("Counter", None, Preset::KoTally.graph()).unwrap();
        let mut json: serde_json::Value =
            serde_json::from_str(&template.encode().unwrap()).unwrap();
        json["schema_version"] = 2.into();
        assert!(BehaviorTemplate::decode(&json.to_string())
            .unwrap_err()
            .contains("schema version"));
        json["schema_version"] = 1.into();
        json["name"] = "x".repeat(MAX_NAME_CHARS + 1).into();
        assert!(BehaviorTemplate::decode(&json.to_string())
            .unwrap_err()
            .contains("name"));
        json["name"] = "valid".into();
        json["description"] = "x".repeat(MAX_DESCRIPTION_CHARS + 1).into();
        assert!(BehaviorTemplate::decode(&json.to_string())
            .unwrap_err()
            .contains("description"));
        assert!(
            BehaviorTemplate::decode(&format!("{} ", " ".repeat(MAX_DOCUMENT_BYTES)))
                .unwrap_err()
                .contains("exceeds")
        );
        assert!(BehaviorTemplate::decode("{\"schema_version\":1,\"name\":\"x\",\"description\":\"\",\"source_fighter\":null,\"graph\":{\"initial\":NaN}}").is_err());
        assert!(BehaviorTemplate::from_graph("bad", None, BehaviorGraph::default()).is_err());
    }

    #[test]
    fn encode_refuses_documents_that_its_decoder_cannot_accept() {
        let mut template =
            BehaviorTemplate::from_graph("Counter", None, Preset::KoTally.graph()).unwrap();
        template.graph.states.push(StateDef {
            status: None,
            name: "unused".repeat(MAX_DOCUMENT_BYTES / 6 + 1),
        });
        assert!(template.encode().unwrap_err().contains("exceeds"));
    }

    #[test]
    fn portable_graphs_cannot_claim_fighter_specific_sources_or_actions() {
        let mut graph = Preset::KoTally.graph();
        graph.transitions[0].actions.push(Action::GenerateArticle {
            symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_FIRE".into(),
        });
        assert!(BehaviorTemplate::from_graph("donor", None, graph)
            .unwrap_err()
            .contains("portable template"));
    }

    #[test]
    fn donor_native_actions_and_status_proofs_reject_wrong_fighters() {
        let article = super::super::registry::entries_for(
            "cloud",
            super::super::registry::IdentifierKind::Article,
        )
        .next()
        .expect("Cloud has an allowlisted article")
        .symbol;
        let mut graph = Preset::KoTally.graph();
        graph.transitions[0].actions.push(Action::GenerateArticle {
            symbol: article.into(),
        });
        let template = BehaviorTemplate::from_graph("Cloud donor", Some("cloud"), graph).unwrap();
        let error = template.instantiate("mario").unwrap_err();
        assert!(error.contains(article), "{error}");
        assert!(template.instantiate("cloud").is_ok());

        let status = super::super::registry::entries_for(
            "cloud",
            super::super::registry::IdentifierKind::FighterStatus,
        )
        .find(|entry| entry.group != "common")
        .expect("Cloud has an allowlisted fighter status")
        .symbol;
        let mut status_graph = Preset::KoTally.graph();
        status_graph.emission_plan.bindings[0].hook = HookName::OnSpend;
        status_graph
            .emission_plan
            .status_triggers
            .push(super::super::codegen::StatusTrigger {
                hook: HookName::OnSpend,
                symbol: status.into(),
                evidence: "test evidence".into(),
                runtime_validated: true,
                transitions: Vec::new(),
            });
        let status_template =
            BehaviorTemplate::from_graph("Cloud status", Some("cloud"), status_graph).unwrap();
        assert!(status_template.instantiate("mario").is_err());
        assert!(status_template.instantiate("cloud").is_ok());
    }

    #[test]
    fn copy_returns_an_independent_validated_graph() {
        let source = Preset::Charge.graph();
        let mut copy = copy_for_fighter(&source, "mario", "cloud").unwrap();
        assert_eq!(copy, source);
        copy.vars[0].initial = ConstValue::Float(42.0);
        assert_eq!(source.vars[0].initial, ConstValue::Float(0.0));
    }
}
