//! Deterministic behavior emitter (slice F2.3).
//!
//! [`emit`] turns a validated [`BehaviorGraph`](super::behavior::BehaviorGraph)
//! plus an [`EmissionPlan`] into Rust source: one state struct with an
//! `initial()` constructor and one pure-logic function per bound hook.
//! The output is deterministic by construction — declaration order in,
//! declaration order out, no hash-map iteration anywhere — so repeated
//! exports are byte-identical and golden tests pin the exact text.
//!
//! Two deliberate boundaries keep the emitter honest:
//!
//! * Hooks resolve through [`super::hooks::resolve`]: unknown or unavailable
//!   hooks fail the plan instead of emitting guessed wiring.
//! * Engine bindings (status numbers, article ids) are **not** emitted.
//!   Generated functions read and write only the state struct; a `BINDING`
//!   comment per hook lists what the wiring slice must connect. Nothing here
//!   invents a status number, and no `unsafe` is needed because no engine
//!   calls are generated. Verification and compilation belong to F2.5; plan
//!   persistence to a follow-up — this slice is validation plus text.

use std::fmt;

use super::behavior::{Action, BehaviorGraph, CmpOp, ConstValue, Expr, Predicate, VarType};
use super::hooks::{self, HookName};
use serde::{Deserialize, Serialize};

pub const MAX_EVENT_BINDINGS: usize = 32;
const MAX_TRANSITIONS_PER_BINDING: usize = super::live_model::MAX_TRANSITIONS;
const MAX_PLAN_BINDINGS: usize = super::live_model::MAX_BINDINGS;
const MAX_STATUS_TRIGGERS: usize = super::live_model::MAX_BINDINGS;
const MAX_RESOURCE_TRIGGERS: usize = super::live_model::MAX_BINDINGS;
const MAX_STATUS_EVIDENCE_BYTES: usize = 4_096;

/// Which transitions each hook evaluates, by index into
/// [`BehaviorGraph::transitions`]. Index order inside one binding is emission
/// order; a transition bound to no hook is dead code and fails the plan, and
/// a transition named twice in one binding would evaluate twice per firing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookBinding {
    pub hook: HookName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub article: Option<String>,
    pub transitions: Vec<usize>,
}

/// Named event receiver and its ordered transition indices; saved and sent as is.
pub use super::live_model::EventBinding;

/// Per-fighter semantic proof for a hook driven by an exact fighter status.
/// A symbol alone proves only identity; export also requires a human-readable
/// evidence note and an explicit result from the runtime validation matrix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusTrigger {
    pub hook: HookName,
    pub symbol: String,
    pub evidence: String,
    #[serde(default)]
    pub runtime_validated: bool,
    /// Optional scoped transition list. Empty remains the legacy/global form
    /// for OnSpend; status-edge hooks must always select exact transitions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<usize>,
}

/// Reviewed built-in resource observations. A project selects a capability, never raw Work IDs,
/// motion hashes, or a self-asserted validation flag.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceSource {
    #[default]
    CloudLimitGroundedNeutral,
    CloudLimitIdleExpiry,
}

impl ResourceSource {
    pub fn fighter(self) -> &'static str {
        "cloud"
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::CloudLimitGroundedNeutral => "Limit Blade Beam (grounded)",
            Self::CloudLimitIdleExpiry => "Limit expiry (standing idle)",
        }
    }
    pub fn runtime_hook(self) -> &'static str {
        match self {
            Self::CloudLimitGroundedNeutral => "CloudLimitGroundedNeutralSpend",
            Self::CloudLimitIdleExpiry => "CloudLimitIdleExpiry",
        }
    }
    pub fn hook(self) -> HookName {
        match self {
            Self::CloudLimitGroundedNeutral => HookName::OnSpend,
            Self::CloudLimitIdleExpiry => HookName::OnTimeout,
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::CloudLimitGroundedNeutral => "Powered neutral special observed grounded from cast entry through resource clear; airborne starts, situation changes, expiry, and stock loss are excluded.",
            Self::CloudLimitIdleExpiry => "Native Limit timer expiry while standing idle; specials, charging, other motions, and stock loss are excluded. This is not an authored timer.",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceTrigger {
    pub hook: HookName,
    pub source: ResourceSource,
}

/// The emission input beside the graph. Travels with its graph: indices only
/// make sense against the transition list they were authored against.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmissionPlan {
    pub bindings: Vec<HookBinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub event_bindings: Vec<EventBinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status_triggers: Vec<StatusTrigger>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_triggers: Vec<ResourceTrigger>,
}

impl EmissionPlan {
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
            && self.event_bindings.is_empty()
            && self.status_triggers.is_empty()
            && self.resource_triggers.is_empty()
    }
}

/// One emission failure: what failed and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitError {
    pub item: String,
    pub reason: String,
}

impl fmt::Display for EmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "codegen {}: {}", self.item, self.reason)
    }
}

/// Validate the plan against the graph: every hook must resolve to a
/// supported hook, every index must address a transition, every binding must
/// evaluate something, no binding may evaluate a transition twice, and every
/// transition must be evaluated somewhere.
pub fn check_plan(graph: &BehaviorGraph, plan: &EmissionPlan) -> Vec<EmitError> {
    let mut errors = Vec::new();
    // Plan validation and subsequent DTO/source lowering must be bounded
    // before walking the graph or copying any user-controlled labels.
    if let Err(reason) = super::behavior::preflight_bounds(graph) {
        errors.push(EmitError {
            item: "behavior graph".into(),
            reason,
        });
        return errors;
    }
    if plan.event_bindings.len() > MAX_EVENT_BINDINGS {
        errors.push(EmitError {
            item: "event bindings".into(),
            reason: format!("event bindings exceed the limit of {MAX_EVENT_BINDINGS}"),
        });
        return errors;
    }
    if plan.bindings.len() > MAX_PLAN_BINDINGS
        || plan.status_triggers.len() > MAX_STATUS_TRIGGERS
        || plan.resource_triggers.len() > MAX_RESOURCE_TRIGGERS
        || plan.event_bindings.len() > MAX_EVENT_BINDINGS
    {
        errors.push(EmitError {
            item: "emission plan".into(),
            reason: format!(
                "hook/status/resource/event table exceeds limits ({MAX_PLAN_BINDINGS}/{MAX_STATUS_TRIGGERS}/{MAX_RESOURCE_TRIGGERS}/{MAX_EVENT_BINDINGS})"
            ),
        });
        return errors;
    }
    if plan
        .bindings
        .iter()
        .any(|binding| binding.transitions.len() > MAX_TRANSITIONS_PER_BINDING)
    {
        errors.push(EmitError {
            item: "emission plan".into(),
            reason: format!(
                "transition binding exceeds the limit of {MAX_TRANSITIONS_PER_BINDING}"
            ),
        });
        return errors;
    }
    if plan
        .status_triggers
        .iter()
        .any(|trigger| trigger.transitions.len() > MAX_TRANSITIONS_PER_BINDING)
    {
        errors.push(EmitError {
            item: "status triggers".into(),
            reason: format!(
                "transition selection exceeds the limit of {MAX_TRANSITIONS_PER_BINDING}"
            ),
        });
        return errors;
    }
    if plan
        .event_bindings
        .iter()
        .any(|binding| binding.transitions.len() > MAX_TRANSITIONS_PER_BINDING)
    {
        errors.push(EmitError {
            item: "event bindings".into(),
            reason: format!(
                "event transition binding exceeds the limit of {MAX_TRANSITIONS_PER_BINDING}"
            ),
        });
        return errors;
    }
    if plan
        .event_bindings
        .iter()
        .any(|binding| binding.fields.len() > super::event_payload::MAX_FIELDS_PER_EVENT)
    {
        errors.push(EmitError {
            item: "emission plan".into(),
            reason: format!(
                "event payload has too many fields; maximum is {}",
                super::event_payload::MAX_FIELDS_PER_EVENT
            ),
        });
        return errors;
    }
    if plan.event_bindings.iter().any(|binding| {
        binding.name.len() > super::live_model::MAX_NAME_BYTES || binding.name.contains('\0')
    }) {
        errors.push(EmitError {
            item: "event bindings".into(),
            reason: format!(
                "event name exceeds the {}-byte limit or contains NUL",
                super::live_model::MAX_NAME_BYTES
            ),
        });
        return errors;
    }
    if !plan_labels_bounded(plan) {
        errors.push(EmitError {
            item: "emission plan".into(),
            reason: "contains an oversized event or status label".into(),
        });
        return errors;
    }
    // Event reads are resolved by behavior validation against the graph's
    // persisted plan. The supplied lowering plan must describe those same
    // scopes; otherwise accepting a clone with different owners could validate
    // one receiver and dispatch another. Equality happens only after table,
    // transition-list, and label bounds make it cheap and finite.
    if graph_uses_event_read(graph) {
        if !plan_shape_bounded(plan)
            || !plan_shape_bounded(&graph.emission_plan)
            || !plan_labels_bounded(plan)
            || !plan_labels_bounded(&graph.emission_plan)
        {
            errors.push(EmitError {
                item: "event read scopes".into(),
                reason: "cannot compare an emission plan containing oversized labels".into(),
            });
            return errors;
        }
        if plan != &graph.emission_plan {
            errors.push(EmitError {
                item: "event read scopes".into(),
                reason: "supplied emission plan conflicts with the graph's authoritative event-read scopes".into(),
            });
            return errors;
        }
    }
    if plan.bindings.is_empty()
        && plan.event_bindings.is_empty()
        && !graph.states.iter().any(|s| s.status.is_some())
    {
        errors.push(EmitError {
            item: "plan".to_string(),
            reason: "no hook or named event bindings; no transition would ever be evaluated"
                .to_string(),
        });
    }
    let mut bound = vec![false; graph.transitions.len()];
    let mut hooks_seen = std::collections::BTreeSet::new();
    let mut events_seen = std::collections::BTreeMap::new();
    let mut triggers = std::collections::BTreeSet::new();
    let mut selected_status_transitions = std::collections::BTreeSet::new();
    for trigger in &plan.status_triggers {
        let item = format!("status trigger {}", trigger.hook.name());
        let identity = if matches!(
            trigger.hook,
            HookName::OnStatusEnter | HookName::OnStatusExit
        ) {
            (trigger.hook, Some(trigger.symbol.as_str()))
        } else {
            (trigger.hook, None)
        };
        if !triggers.insert(identity) {
            errors.push(EmitError {
                item: item.clone(),
                reason: "the hook/status pair has more than one status trigger".to_string(),
            });
            continue;
        }
        if !matches!(
            hooks::lookup(trigger.hook).state,
            hooks::SupportState::StatusBound { .. }
        ) {
            errors.push(EmitError {
                item: item.clone(),
                reason: "this hook is not status-bound and cannot carry a status trigger"
                    .to_string(),
            });
        }
        if trigger.symbol.trim().is_empty() {
            errors.push(EmitError {
                item: item.clone(),
                reason: "the exact fighter-status symbol is empty".to_string(),
            });
        }
        let is_status_edge = matches!(
            trigger.hook,
            HookName::OnStatusEnter | HookName::OnStatusExit
        );
        if trigger.hook == HookName::OnSpend && !trigger.transitions.is_empty() {
            errors.push(EmitError {
                item: item.clone(),
                reason: "transition selection is supported only for onStatusEnter/onStatusExit"
                    .into(),
            });
        }
        let requires_semantic_review = !is_status_edge;
        if requires_semantic_review && trigger.evidence.trim().is_empty() {
            errors.push(EmitError {
                item: item.clone(),
                reason: "no review evidence is recorded".to_string(),
            });
        }
        if requires_semantic_review && !trigger.runtime_validated {
            errors.push(EmitError {
                item: item.clone(),
                reason: "the status edge has not been marked runtime validated".to_string(),
            });
        }
        if is_status_edge {
            if trigger.transitions.is_empty() {
                errors.push(EmitError {
                    item: item.clone(),
                    reason: format!(
                        "{} must select at least one transition",
                        trigger.hook.name()
                    ),
                });
            }
            let binding = plan
                .bindings
                .iter()
                .find(|binding| binding.hook == trigger.hook);
            if trigger.transitions.len() > MAX_TRANSITIONS_PER_BINDING {
                errors.push(EmitError {
                    item: item.clone(),
                    reason: format!(
                        "transition selection exceeds the limit of {MAX_TRANSITIONS_PER_BINDING}"
                    ),
                });
            } else {
                for index in &trigger.transitions {
                    if *index >= graph.transitions.len() {
                        errors.push(EmitError {
                            item: item.clone(),
                            reason: format!("transition index {index} is outside the graph"),
                        });
                    } else if !binding.is_some_and(|binding| {
                        binding.transitions.len() <= MAX_TRANSITIONS_PER_BINDING
                            && binding.transitions.contains(index)
                    }) {
                        errors.push(EmitError {
                            item: item.clone(),
                            reason: format!(
                                "transition index {index} is not in the {} hook binding",
                                trigger.hook.name()
                            ),
                        });
                    } else if !selected_status_transitions.insert(*index) {
                        errors.push(EmitError {
                            item: item.clone(),
                            reason: format!(
                                "transition index {index} is selected by more than one status"
                            ),
                        });
                    } else if !graph.transitions[*index].on_frame {
                        errors.push(EmitError {
                            item: item.clone(),
                            reason: format!(
                                "transition index {index} must declare an event boundary"
                            ),
                        });
                    }
                }
            }
        }
    }
    let mut resource_hooks = std::collections::BTreeSet::new();
    for trigger in &plan.resource_triggers {
        let item = format!("resource trigger {}", trigger.hook.name());
        if trigger.hook != trigger.source.hook() {
            errors.push(EmitError {
                item: item.clone(),
                reason: format!(
                    "this resource source can only bind {}",
                    trigger.source.hook().name()
                ),
            });
        }
        if !resource_hooks.insert(trigger.hook)
            || triggers.iter().any(|(hook, _)| *hook == trigger.hook)
        {
            errors.push(EmitError {
                item,
                reason: "a hook must have exactly one status or resource source".into(),
            });
        }
    }
    let mut transitions_seen = std::collections::BTreeSet::new();
    for binding in &plan.bindings {
        if binding.article.is_some()
            && !matches!(
                binding.hook,
                HookName::OnArticleSpawn | HookName::OnArticleRemoved
            )
        {
            errors.push(EmitError {
                item: "article filter".into(),
                reason: "article filters require an article lifecycle hook".into(),
            });
        }
        let item = format!("binding {}", binding.hook.name());
        if !hooks_seen.insert(binding.hook) {
            errors.push(EmitError {
                item: item.clone(),
                reason: "the hook is bound more than once".to_string(),
            });
        }
        if let Err(error) = hooks::resolve_binding(
            binding.hook.name(),
            triggers.iter().any(|(hook, _)| *hook == binding.hook)
                || resource_hooks.contains(&binding.hook),
        ) {
            errors.push(EmitError {
                item: item.clone(),
                reason: error.to_string(),
            });
        }
        if binding.transitions.is_empty() {
            errors.push(EmitError {
                item: item.clone(),
                reason: "binds no transitions; an empty binding evaluates nothing".to_string(),
            });
        }
        if binding.transitions.len() > MAX_TRANSITIONS_PER_BINDING {
            errors.push(EmitError {
                item: item.clone(),
                reason: format!(
                    "transition binding exceeds the limit of {MAX_TRANSITIONS_PER_BINDING}"
                ),
            });
        } else {
            let mut seen = std::collections::BTreeSet::new();
            for index in &binding.transitions {
                if !seen.insert(index) {
                    errors.push(EmitError {
                        item: item.clone(),
                        reason: format!("evaluates transition {index} twice per firing"),
                    });
                }
                if !transitions_seen.insert(*index) {
                    errors.push(EmitError {
                        item: item.clone(),
                        reason: format!("transition {index} is also bound to another hook"),
                    });
                }
                if *index >= graph.transitions.len() {
                    errors.push(EmitError {
                        item: item.clone(),
                        reason: format!(
                            "addresses transition {index}, but the graph holds only {}",
                            graph.transitions.len()
                        ),
                    });
                } else {
                    bound[*index] = true;
                }
            }
        }
    }
    for (binding_index, binding) in plan.event_bindings.iter().enumerate() {
        // Use the bounded index as the diagnostic label until the authored
        // name has passed its byte-limit check below.
        let item = format!("event binding {binding_index}");
        errors.extend(
            super::event_payload::validate_schema("behavior", &binding.name, &binding.fields)
                .err()
                .unwrap_or_default()
                .into_iter()
                .map(|error| EmitError {
                    item: item.clone(),
                    reason: error.to_string(),
                }),
        );
        if binding.name.is_empty() || binding.name.len() > 48 || binding.name.contains('\0') {
            errors.push(EmitError {
                item: item.clone(),
                reason: "event name must be non-empty, NUL-free, and at most 48 bytes".into(),
            });
        }
        if events_seen.insert(binding.name.as_str(), binding).is_some() {
            errors.push(EmitError {
                item: item.clone(),
                reason: "the named event has more than one receiver binding".into(),
            });
        }
        if binding.transitions.is_empty() {
            errors.push(EmitError {
                item: item.clone(),
                reason: "binds no transitions".into(),
            });
        }
        if binding.transitions.len() > MAX_TRANSITIONS_PER_BINDING {
            errors.push(EmitError {
                item: item.clone(),
                reason: format!(
                    "event transition binding exceeds the limit of {MAX_TRANSITIONS_PER_BINDING}"
                ),
            });
        } else {
            let mut seen = std::collections::BTreeSet::new();
            for index in &binding.transitions {
                if !seen.insert(index) {
                    errors.push(EmitError {
                        item: item.clone(),
                        reason: format!("evaluates transition {index} twice per event"),
                    });
                }
                if !transitions_seen.insert(*index) {
                    errors.push(EmitError {
                        item: item.clone(),
                        reason: format!("transition {index} is also bound to another source"),
                    });
                }
                if *index >= graph.transitions.len() {
                    errors.push(EmitError {
                        item: item.clone(),
                        reason: format!(
                            "addresses transition {index}, but the graph holds only {}",
                            graph.transitions.len()
                        ),
                    });
                } else {
                    bound[*index] = true;
                }
            }
        }
    }
    for (transition_index, transition) in graph.transitions.iter().enumerate() {
        for (action_index, action) in transition.actions.iter().enumerate() {
            let item = format!("transition {transition_index} action {action_index}");
            match action {
                Action::EmitEvent { name } => match events_seen.get(name.as_str()) {
                    None => errors.push(EmitError {
                        item,
                        reason: "emits an unbound named event".into(),
                    }),
                    Some(binding) if !binding.fields.is_empty() => errors.push(EmitError {
                        item,
                        reason: "name-only EmitEvent cannot target a receiver with fields; provide explicit typed arguments".into(),
                    }),
                    Some(_) => {}
                },
                Action::EmitEventArguments { name, arguments } => {
                    if let Some(binding) = events_seen.get(name.as_str()) {
                        if binding.fields.is_empty() {
                            errors.push(EmitError {
                                item: item.clone(),
                                reason: "EmitEventArguments requires a nonempty receiver schema; use legacy EmitEvent for zero-field receivers".into(),
                            });
                        } else if let Err(argument_errors) = super::event_payload::canonicalize_arguments(
                            &item,
                            name,
                            &binding.fields,
                            arguments,
                        ) {
                            errors.extend(argument_errors.into_iter().map(|error| EmitError {
                                item: item.clone(),
                                reason: error.to_string(),
                            }));
                        }
                    } else {
                        errors.push(EmitError {
                            item: item.clone(),
                            reason: "emits an unbound named event".into(),
                        });
                    }
                }
                Action::EmitEventValues { name, arguments } => {
                    if let Some(binding) = events_seen.get(name.as_str()) {
                        if binding.fields.is_empty() {
                            errors.push(EmitError {
                                item: item.clone(),
                                reason: "EmitEventValues requires a nonempty receiver schema; use legacy EmitEvent for zero-field receivers".into(),
                            });
                        } else if let Err(argument_errors) =
                            super::event_payload::canonicalize_value_arguments(
                                &item,
                                name,
                                &binding.fields,
                                arguments,
                                |value| {
                                    super::behavior::event_value_kind(
                                        graph,
                                        transition_index,
                                        value,
                                    )
                                },
                            )
                        {
                            errors.extend(argument_errors.into_iter().map(|error| EmitError {
                                item: item.clone(),
                                reason: error.to_string(),
                            }));
                        }
                    } else {
                        errors.push(EmitError {
                            item: item.clone(),
                            reason: "emits an unbound named event".into(),
                        });
                    }
                }
                _ => {}
            }
        }
    }
    for trigger in &plan.status_triggers {
        if !hooks_seen.contains(&trigger.hook) {
            errors.push(EmitError {
                item: format!("status trigger {}", trigger.hook.name()),
                reason: "the trigger has no matching hook binding".to_string(),
            });
        }
    }
    for hook in [HookName::OnStatusEnter, HookName::OnStatusExit] {
        if let Some(binding) = plan.bindings.iter().find(|binding| binding.hook == hook) {
            if binding.transitions.len() <= MAX_TRANSITIONS_PER_BINDING {
                for index in &binding.transitions {
                    if !selected_status_transitions.contains(index) {
                        errors.push(EmitError {
                            item: format!("binding {}", hook.name()),
                            reason: format!(
                                "transition index {index} is not selected by an exact status trigger"
                            ),
                        });
                    }
                }
            }
        }
    }
    for trigger in &plan.resource_triggers {
        if !hooks_seen.contains(&trigger.hook) {
            errors.push(EmitError {
                item: format!("resource trigger {}", trigger.hook.name()),
                reason: "the trigger has no matching hook binding".into(),
            });
        }
    }
    for state in &graph.states {
        if let Some(binding) = &state.status {
            for index in binding.roots.values().flatten() {
                if let Some(bound) = bound.get_mut(*index) {
                    *bound = true;
                }
            }
        }
    }
    for (index, evaluated) in bound.iter().enumerate() {
        if !evaluated {
            errors.push(EmitError {
                item: format!("transition {index}"),
                reason: "bound to no hook; it would be dead code".to_string(),
            });
        }
    }
    errors
}

fn plan_shape_bounded(plan: &EmissionPlan) -> bool {
    plan.bindings.len() <= MAX_PLAN_BINDINGS
        && plan.status_triggers.len() <= MAX_STATUS_TRIGGERS
        && plan.resource_triggers.len() <= MAX_RESOURCE_TRIGGERS
        && plan.event_bindings.len() <= MAX_EVENT_BINDINGS
        && plan
            .bindings
            .iter()
            .all(|binding| binding.transitions.len() <= MAX_TRANSITIONS_PER_BINDING)
        && plan
            .status_triggers
            .iter()
            .all(|trigger| trigger.transitions.len() <= MAX_TRANSITIONS_PER_BINDING)
        && plan.event_bindings.iter().all(|binding| {
            binding.transitions.len() <= MAX_TRANSITIONS_PER_BINDING
                && binding.fields.len() <= super::live_model::MAX_EVENT_FIELDS
        })
}

fn plan_labels_bounded(plan: &EmissionPlan) -> bool {
    let text_ok = |text: &str, max: usize| text.len() <= max;
    plan.bindings.iter().all(|binding| {
        binding
            .article
            .as_ref()
            .is_none_or(|symbol| text_ok(symbol, super::live_model::MAX_SYMBOL_BYTES))
    }) && plan.event_bindings.iter().all(|binding| {
        text_ok(&binding.name, super::live_model::MAX_NAME_BYTES)
            && binding.fields.len() <= super::live_model::MAX_EVENT_FIELDS
            && binding
                .fields
                .iter()
                .all(|field| text_ok(&field.name, super::live_model::MAX_NAME_BYTES))
    }) && plan.status_triggers.iter().all(|trigger| {
        text_ok(&trigger.symbol, super::live_model::MAX_SYMBOL_BYTES)
            && text_ok(&trigger.evidence, MAX_STATUS_EVIDENCE_BYTES)
    })
}

fn graph_uses_event_read(graph: &BehaviorGraph) -> bool {
    fn expr_uses_event_read(expr: &Expr) -> bool {
        match expr {
            Expr::PendingDamage | Expr::EventField { .. } => true,
            Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
                expr_uses_event_read(left) || expr_uses_event_read(right)
            }
            Expr::StatusFrame
            | Expr::MotionFrame
            | Expr::StickX { .. }
            | Expr::StickY
            | Expr::RandomInt { .. }
            | Expr::RandomFloat { .. }
            | Expr::Const(_)
            | Expr::Var(_)
            | Expr::TimerRemaining(_)
            | Expr::NativeFloat(_)
            | Expr::NativeInt(_) => false,
        }
    }
    fn predicate_uses_event_read(predicate: &Predicate) -> bool {
        match predicate {
            Predicate::DamageIsProjectile => true,
            Predicate::InStatus { .. }
            | Predicate::InMotion { .. }
            | Predicate::SituationChanged
            | Predicate::MotionEnded
            | Predicate::CancelEnabled
            | Predicate::TermEnabled { .. }
            | Predicate::Situation { .. }
            | Predicate::Button { .. } => false,
            Predicate::EventFlag { .. } | Predicate::DamageStatusIs(_) => true,
            Predicate::Compare { left, right, .. } => {
                expr_uses_event_read(left) || expr_uses_event_read(right)
            }
            Predicate::Not(inner) => predicate_uses_event_read(inner),
            Predicate::And(left, right) | Predicate::Or(left, right) => {
                predicate_uses_event_read(left) || predicate_uses_event_read(right)
            }
            Predicate::Flag(_) | Predicate::NativeFlag(_) | Predicate::Expired(_) => false,
        }
    }
    graph.transitions.iter().any(|transition| {
        predicate_uses_event_read(&transition.guard)
            || transition
                .actions
                .iter()
                .flat_map(Action::operands)
                .any(|operand| match operand {
                    super::live_model::Operand::Expr(e) => expr_uses_event_read(e),
                    super::live_model::Operand::Predicate(p) => predicate_uses_event_read(p),
                })
    })
}

/// Validate exact status ownership against the pinned registry. Kept separate
/// from structural plan checks because ownership requires the project fighter.
pub fn check_status_triggers(fighter: &str, plan: &EmissionPlan) -> Vec<EmitError> {
    if !plan_shape_bounded(plan) || !plan_labels_bounded(plan) {
        return vec![EmitError {
            item: "emission plan".into(),
            reason: "exceeds structural limits or contains an oversized label".into(),
        }];
    }
    let mut errors: Vec<_> = plan
        .status_triggers
        .iter()
        .filter_map(|trigger| {
            super::registry::resolve(
                fighter,
                &trigger.symbol,
                super::registry::IdentifierKind::FighterStatus,
            )
            .err()
            .map(|error| EmitError {
                item: format!("status trigger {}", trigger.hook.name()),
                reason: error.to_string(),
            })
        })
        .collect();
    for binding in &plan.bindings {
        if let Some(symbol) = &binding.article {
            if let Err(reason) =
                super::registry::resolve(fighter, symbol, super::registry::IdentifierKind::Article)
            {
                errors.push(EmitError {
                    item: "article filter".into(),
                    reason: reason.to_string(),
                });
            }
        }
    }
    errors.extend(
        plan.resource_triggers
            .iter()
            .filter(|trigger| trigger.source.fighter() != fighter)
            .map(|trigger| EmitError {
                item: format!("resource trigger {}", trigger.hook.name()),
                reason: format!(
                    "{} belongs to {}, not {fighter}",
                    trigger.source.label(),
                    trigger.source.fighter()
                ),
            }),
    );
    errors
}

/// Emit the state struct plus one function per binding.
///
/// Fails closed on an empty graph (nothing to emit — the caller skips those),
/// an empty fighter name, an invalid graph, or an invalid plan. Every error
/// names its item; a returned string is complete source.
pub fn emit(
    fighter: &str,
    graph: &BehaviorGraph,
    plan: &EmissionPlan,
) -> Result<String, Vec<EmitError>> {
    if let Err(reason) = super::behavior::preflight_bounds(graph) {
        return Err(vec![EmitError {
            item: "behavior graph".into(),
            reason,
        }]);
    }
    let mut errors: Vec<EmitError> = super::behavior::validate(graph)
        .into_iter()
        .map(|error| EmitError {
            item: error.item,
            reason: error.reason,
        })
        .collect();
    errors.extend(check_plan(graph, plan));
    errors.extend(check_status_triggers(fighter, plan));
    if graph_uses_native_input(graph) || super::behavior::requires_controller_runtime(graph) {
        errors.push(EmitError {
            item: "native inputs".into(),
            reason: "the pure-state source emitter has no dispatch snapshot; use the static adapter or live behavior runtime".into(),
        });
    }
    if !plan.event_bindings.is_empty()
        || graph.transitions.iter().any(|transition| {
            transition
                .actions
                .iter()
                .any(|action| matches!(action, Action::EmitEvent { .. }))
                || transition.actions.iter().any(|action| {
                    matches!(
                        action,
                        Action::EmitEventArguments { .. } | Action::EmitEventValues { .. }
                    )
                })
        })
    {
        errors.push(EmitError {
            item: "named events".into(),
            reason: "the pure-state source emitter cannot dispatch named events; use the static adapter or live behavior runtime".into(),
        });
    }
    if graph.is_empty() {
        errors.push(EmitError {
            item: "graph".to_string(),
            reason: "no behavior authored; nothing to emit".to_string(),
        });
    }
    if sanitize(fighter).is_empty() {
        errors.push(EmitError {
            item: "fighter".to_string(),
            reason: "the fighter name is empty; generated items need a name".to_string(),
        });
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut scope = Scope::new(fighter, graph);
    let mut out = String::new();
    out.push_str("// Generated by Visionary (gimmicks F2.3) from fighter \"");
    out.push_str(fighter);
    out.push_str(
        "\" behavior.\n// Do not edit: re-export overwrites this file.\n#![allow(dead_code)]\n\n",
    );
    out.push_str("/// Behavior state for fighter \"");
    out.push_str(fighter);
    out.push_str(
        "\". `state_id` is the index\n/// of the current state in declaration order; the rest are the graph\n/// variables in declaration order.\n",
    );
    out.push_str(&format!("pub struct {} {{\n", scope.state_struct));
    out.push_str("    pub state_id: i32,\n");
    for entry in &scope.vars {
        out.push_str(&format!(
            "    pub {}: {},\n",
            entry.ident,
            rust_type(entry.kind)
        ));
    }
    out.push_str("}\n\n");
    out.push_str(&format!("impl {} {{\n", scope.state_struct));
    out.push_str("    pub fn initial() -> Self {\n");
    out.push_str(&format!(
        "        Self {{\n            state_id: {},\n",
        scope.initial_id
    ));
    for entry in &scope.vars {
        out.push_str(&format!(
            "            {}: {},\n",
            entry.ident,
            literal(&entry.initial)
        ));
    }
    out.push_str("        }\n    }\n}\n");
    for binding in &plan.bindings {
        let hook_fn = scope.hook_fn(&binding.hook);
        out.push_str(&format!(
            "\n/// Evaluated on {}. BINDING: connect this to the {:?} entry point.\n",
            binding.hook.name(),
            plan.resource_triggers
                .iter()
                .find(|trigger| trigger.hook == binding.hook)
                .map_or_else(
                    || hooks::lookup(binding.hook).state_entry_debug(),
                    |trigger| trigger.source.runtime_hook(),
                )
        ));
        out.push_str(&format!(
            "pub fn {hook_fn}(state: &mut {}) {{\n",
            scope.state_struct
        ));
        for index in &binding.transitions {
            let transition = &graph.transitions[*index];
            let from_id = scope.state_id(&transition.from);
            let guard = predicate(&transition.guard, &scope);
            out.push_str(&format!(
                "    if state.state_id == {from_id} && {guard} {{\n"
            ));
            for action in &transition.actions {
                out.push_str(&format!("        {};\n", action_text(action, &scope)));
            }
            out.push_str(&format!(
                "        state.state_id = {};\n    }}\n",
                scope.state_id(&transition.to)
            ));
        }
        out.push_str("}\n");
    }
    Ok(out)
}

/// Debug view of a hook's entry point for the BINDING comment. Kept beside
/// the emitter (not in `hooks`) because it is presentation, not registry.
trait BindingComment {
    fn state_entry_debug(&self) -> &'static str;
}

impl BindingComment for super::hooks::HookDef {
    fn state_entry_debug(&self) -> &'static str {
        match &self.state {
            super::hooks::SupportState::Supported { entry, .. }
            | super::hooks::SupportState::StatusBound { entry, .. } => match entry {
                super::hooks::EntryPoint::AttackerCollisionQueue => "AttackerCollisionQueue",
                super::hooks::EntryPoint::DefenderCollisionQueue => "DefenderCollisionQueue",
                super::hooks::EntryPoint::FrameTick => "FrameTick",
                super::hooks::EntryPoint::TimerExpiry => "TimerExpiry",
                super::hooks::EntryPoint::FighterSpawn => "FighterSpawn",
                super::hooks::EntryPoint::FighterDown => "FighterDown",
                super::hooks::EntryPoint::StatusChange => "StatusChange",
                super::hooks::EntryPoint::WeaponSpawn => "WeaponSpawn",
            },
            super::hooks::SupportState::Unavailable { .. } => "unavailable",
        }
    }
}

/// Collision-safe naming scope: every generated identifier derives from one
/// sanitized fighter prefix plus sanitized parts, deduplicated with numeric
/// suffixes so `a-b` and `a_b` (or two identical labels) never collide.
struct Scope {
    tag: String,
    state_struct: String,
    vars: Vec<VarEntry>,
    state_ids: Vec<(String, usize)>,
    initial_id: usize,
    taken: std::collections::BTreeSet<String>,
    hook_names: std::collections::BTreeMap<HookName, String>,
}

struct VarEntry {
    name: String,
    ident: String,
    kind: VarType,
    initial: ConstValue,
}

impl Scope {
    fn new(fighter: &str, graph: &BehaviorGraph) -> Self {
        let tag = sanitize(fighter);
        let state_struct = format!("Gimmick{tag}State");
        let mut taken = std::collections::BTreeSet::from([state_struct.clone()]);
        let mut vars = Vec::new();
        for var in &graph.vars {
            vars.push(VarEntry {
                ident: unique(&mut taken, &sanitize(&var.name)),
                name: var.name.clone(),
                kind: var.kind,
                initial: initial_value(var),
            });
        }
        let state_ids = graph
            .states
            .iter()
            .enumerate()
            .map(|(index, state)| (state.name.clone(), index))
            .collect::<Vec<_>>();
        let initial_id = state_ids
            .iter()
            .find(|(name, _)| name == &graph.initial)
            .map(|(_, index)| *index)
            .unwrap_or(0);
        Self {
            tag,
            state_struct,
            vars,
            state_ids,
            initial_id,
            taken,
            hook_names: std::collections::BTreeMap::new(),
        }
    }

    fn state_id(&self, state: &str) -> usize {
        self.state_ids
            .iter()
            .find(|(name, _)| name == state)
            .map(|(_, index)| *index)
            .unwrap_or(0)
    }

    fn var(&self, name: &str) -> Option<&VarEntry> {
        self.vars.iter().find(|entry| entry.name == name)
    }

    fn var_ident(&self, name: &str) -> String {
        // The validator guarantees declared names; the fallback keeps the
        // emitter total so an unchecked caller still gets text, not a panic.
        self.var(name)
            .map(|entry| entry.ident.clone())
            .unwrap_or_else(|| sanitize(name))
    }

    fn hook_fn(&mut self, hook: &HookName) -> String {
        if let Some(name) = self.hook_names.get(hook) {
            return name.clone();
        }
        let base = format!("gimmick_{}_{}", self.tag, snake(hook.name()));
        let name = unique(&mut self.taken, &base);
        self.hook_names.insert(*hook, name.clone());
        name
    }
}

fn rust_type(kind: VarType) -> &'static str {
    match kind {
        VarType::Float => "f32",
        VarType::Int | VarType::Timer => "i32",
        VarType::Flag => "bool",
    }
}

fn initial_value(var: &super::behavior::VarDef) -> ConstValue {
    match (var.kind, var.initial) {
        (VarType::Timer, ConstValue::Int(frames)) => ConstValue::Int(frames),
        (_, initial) => initial,
    }
}

fn literal(value: &ConstValue) -> String {
    match value {
        ConstValue::Float(v) => format!("{v:?}"),
        ConstValue::Int(v) => v.to_string(),
        ConstValue::Flag(v) => v.to_string(),
    }
}

fn sanitize(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    while out.starts_with('_') {
        out.remove(0);
    }
    while out.starts_with(|ch: char| ch.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

fn unique(taken: &mut std::collections::BTreeSet<String>, base: &str) -> String {
    let base = if base.is_empty() {
        "v".to_string()
    } else {
        base.to_string()
    };
    if taken.insert(base.clone()) {
        return base;
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{base}_{suffix}");
        if taken.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn snake(camel: &str) -> String {
    // Underscore before an uppercase letter that starts a new word: one
    // preceded by a lowercase/digit, or one followed by a lowercase letter
    // (so "onKO" becomes "on_ko", not "on_k_o").
    let chars: Vec<char> = camel.chars().collect();
    let mut out = String::new();
    for (index, ch) in chars.iter().enumerate() {
        if ch.is_ascii_uppercase() {
            let prev = index.checked_sub(1).and_then(|i| chars.get(i).copied());
            let next = chars.get(index + 1).copied();
            let prev_lower = prev.is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit());
            let next_lower = next.is_some_and(|n| n.is_ascii_lowercase());
            if prev.is_some() && (prev_lower || next_lower) {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else if ch.is_ascii_alphanumeric() || *ch == '_' {
            out.push(*ch);
        } else {
            out.push('_');
        }
    }
    out
}

/// The snake-case hook suffix output functions carry
/// (`gimmick_<fighter>_<suffix>`), for the verifier's plan-vs-output check.
pub(crate) fn hook_snake(hook: &HookName) -> String {
    snake(hook.name())
}

/// The sanitized fighter tag output identifiers carry, for the verifier's
/// plan-vs-output check.
pub(crate) fn fighter_tag(fighter: &str) -> String {
    sanitize(fighter)
}

fn expr_text(expr: &Expr, scope: &Scope) -> String {
    match expr {
        Expr::StatusFrame
        | Expr::MotionFrame
        | Expr::StickX { .. }
        | Expr::StickY
        | Expr::RandomInt { .. }
        | Expr::RandomFloat { .. } => {
            unreachable!("shared controller runtime required")
        }
        Expr::Const(value) => literal(value),
        Expr::Var(name) => format!("state.{}", scope.var_ident(name)),
        Expr::TimerRemaining(name) => format!("state.{}", scope.var_ident(name)),
        Expr::PendingDamage | Expr::EventField { .. } => {
            unreachable!("event reads are rejected before pure-state lowering")
        }
        Expr::NativeFloat(_) | Expr::NativeInt(_) => {
            unreachable!("native inputs are rejected before pure-state lowering")
        }
        Expr::Add(left, right) => {
            format!("({} + {})", expr_text(left, scope), expr_text(right, scope))
        }
        Expr::Sub(left, right) => {
            format!("({} - {})", expr_text(left, scope), expr_text(right, scope))
        }
        Expr::Mul(left, right) => {
            format!("({} * {})", expr_text(left, scope), expr_text(right, scope))
        }
    }
}

fn graph_uses_native_input(graph: &BehaviorGraph) -> bool {
    fn has_native_expr(expression: &Expr) -> bool {
        match expression {
            Expr::StatusFrame
            | Expr::MotionFrame
            | Expr::StickX { .. }
            | Expr::StickY
            | Expr::RandomInt { .. }
            | Expr::RandomFloat { .. } => false,
            Expr::NativeFloat(_) | Expr::NativeInt(_) => true,
            Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
                has_native_expr(left) || has_native_expr(right)
            }
            Expr::PendingDamage | Expr::Const(_) | Expr::Var(_) | Expr::TimerRemaining(_) => false,
            Expr::EventField { .. } => false,
        }
    }
    fn has_native_predicate(value: &Predicate) -> bool {
        match value {
            Predicate::DamageIsProjectile => true,
            Predicate::InStatus { .. }
            | Predicate::InMotion { .. }
            | Predicate::SituationChanged
            | Predicate::MotionEnded
            | Predicate::CancelEnabled
            | Predicate::TermEnabled { .. }
            | Predicate::Situation { .. }
            | Predicate::Button { .. } => false,
            Predicate::Compare { left, right, .. } => {
                has_native_expr(left) || has_native_expr(right)
            }
            Predicate::NativeFlag(_) | Predicate::DamageStatusIs(_) => true,
            Predicate::Not(inner) => has_native_predicate(inner),
            Predicate::And(left, right) | Predicate::Or(left, right) => {
                has_native_predicate(left) || has_native_predicate(right)
            }
            Predicate::Flag(_) | Predicate::Expired(_) => false,
            Predicate::EventFlag { .. } => false,
        }
    }
    graph.transitions.iter().any(|transition| {
        has_native_predicate(&transition.guard)
            || transition
                .actions
                .iter()
                .flat_map(Action::operands)
                .any(|operand| match operand {
                    super::live_model::Operand::Expr(e) => has_native_expr(e),
                    super::live_model::Operand::Predicate(p) => has_native_predicate(p),
                })
    })
}

fn cmp(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "==",
        CmpOp::Lt => "<",
        CmpOp::Lte => "<=",
        CmpOp::Gt => ">",
        CmpOp::Gte => ">=",
    }
}

fn predicate(guard: &Predicate, scope: &Scope) -> String {
    match guard {
        Predicate::DamageIsProjectile
        | Predicate::InStatus { .. }
        | Predicate::InMotion { .. }
        | Predicate::SituationChanged
        | Predicate::MotionEnded
        | Predicate::CancelEnabled
        | Predicate::TermEnabled { .. }
        | Predicate::Situation { .. }
        | Predicate::Button { .. } => {
            unreachable!("shared controller runtime required")
        }
        Predicate::Flag(name) => format!("state.{}", scope.var_ident(name)),
        Predicate::EventFlag { .. } => {
            unreachable!("event reads are rejected before pure-state lowering")
        }
        Predicate::NativeFlag(_) | Predicate::DamageStatusIs(_) => {
            unreachable!("native inputs are rejected before pure-state lowering")
        }
        Predicate::Expired(name) => format!("state.{} <= 0", scope.var_ident(name)),
        Predicate::Compare { op, left, right } => format!(
            "({} {} {})",
            expr_text(left, scope),
            cmp(*op),
            expr_text(right, scope)
        ),
        Predicate::Not(inner) => format!("!({})", predicate(inner, scope)),
        Predicate::And(left, right) => format!(
            "({} && {})",
            predicate(left, scope),
            predicate(right, scope)
        ),
        Predicate::Or(left, right) => format!(
            "({} || {})",
            predicate(left, scope),
            predicate(right, scope)
        ),
    }
}

fn action_text(action: &Action, scope: &Scope) -> String {
    match action {
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
        | Action::ChooseHeroMenuSpell { .. }
        | Action::AddSpeed { .. }
        | Action::Object { .. }
        | Action::SetNative { .. }
        | Action::AddNative { .. }
        | Action::SetNativeFlag { .. } => {
            unreachable!("shared controller runtime required")
        }
        Action::Set { var, value } => {
            format!(
                "state.{} = {}",
                scope.var_ident(var),
                expr_text(value, scope)
            )
        }
        Action::SetFlag { var, value } => {
            format!(
                "state.{} = {}",
                scope.var_ident(var),
                predicate(value, scope)
            )
        }
        Action::Add { var, value } => {
            format!(
                "state.{} += {}",
                scope.var_ident(var),
                expr_text(value, scope)
            )
        }
        Action::Raise { var } => format!("state.{} = true", scope.var_ident(var)),
        Action::Clear { var } => format!("state.{} = false", scope.var_ident(var)),
        Action::Reset { var } => {
            // emit() validates first, so the variable is declared; the expect
            // documents that invariant the way hooks::lookup does.
            let initial = scope
                .var(var)
                .expect("validator guarantees declared variables")
                .initial;
            format!("state.{} = {}", scope.var_ident(var), literal(&initial))
        }
        // F2.3 deliberately remains a pure state-code emitter. The static
        // Skyline adapter lowers symbolic engine actions through its guarded
        // runtime instead; preserve the transition shape here without
        // inventing an engine call or raw identifier in this generic output.
        Action::GenerateArticle { .. }
        | Action::RemoveArticle { .. }
        | Action::RemoveNewestArticle { .. }
        | Action::ArticleMovement { .. }
        | Action::RequestArticleStatus { .. }
        | Action::RequestFighterStatus { .. }
        | Action::RequestWeaponStatus { .. }
        | Action::EmitEvent { .. }
        | Action::EmitEventArguments { .. }
        | Action::EmitEventValues { .. } => "state.state_id = state.state_id".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::behavior::{Transition, VarDef};
    use super::*;
    use crate::gimmicks::fixtures::{charge_graph, charge_plan};

    fn typed_event_graph() -> (BehaviorGraph, EmissionPlan) {
        let mut graph = charge_graph();
        graph.transitions.push(Transition {
            from: "idle".into(),
            to: "idle".into(),
            guard: Predicate::Flag("powered".into()),
            on_frame: true,
            actions: Vec::new(),
        });
        graph.transitions[0]
            .actions
            .push(Action::EmitEventArguments {
                name: "ready".into(),
                arguments: vec![
                    super::super::event_payload::Argument {
                        field: "count".into(),
                        value: ConstValue::Int(3),
                    },
                    super::super::event_payload::Argument {
                        field: "active".into(),
                        value: ConstValue::Flag(true),
                    },
                ],
            });
        let mut plan = charge_plan();
        plan.event_bindings.push(EventBinding {
            name: "ready".into(),
            transitions: vec![2],
            fields: vec![
                super::super::event_payload::FieldDef {
                    name: "active".into(),
                    kind: VarType::Flag,
                },
                super::super::event_payload::FieldDef {
                    name: "count".into(),
                    kind: VarType::Int,
                },
            ],
        });
        graph.emission_plan = plan.clone();
        (graph, plan)
    }

    #[test]
    fn emission_is_pinned_by_a_golden() {
        let source = emit("cloud", &charge_graph(), &charge_plan()).unwrap();
        // Explicit \n escapes throughout: a trailing-\ continuation would
        // strip the leading indentation this golden exists to pin.
        let expected = "// Generated by Visionary (gimmicks F2.3) from fighter \"cloud\" behavior.\n// Do not edit: re-export overwrites this file.\n#![allow(dead_code)]\n\n/// Behavior state for fighter \"cloud\". `state_id` is the index\n/// of the current state in declaration order; the rest are the graph\n/// variables in declaration order.\npub struct GimmickcloudState {\n    pub state_id: i32,\n    pub gauge: f32,\n    pub powered: bool,\n}\n\nimpl GimmickcloudState {\n    pub fn initial() -> Self {\n        Self {\n            state_id: 0,\n            gauge: 0.0,\n            powered: false,\n        }\n    }\n}\n\n/// Evaluated on onChargeTick. BINDING: connect this to the \"FrameTick\" entry point.\npub fn gimmick_cloud_on_charge_tick(state: &mut GimmickcloudState) {\n    if state.state_id == 0 && (state.gauge >= 100.0) {\n        state.powered = true;\n        state.gauge = 0.0;\n        state.state_id = 1;\n    }\n}\n\n/// Evaluated on onTimeout. BINDING: connect this to the \"TimerExpiry\" entry point.\npub fn gimmick_cloud_on_timeout(state: &mut GimmickcloudState) {\n    if state.state_id == 1 && state.powered {\n        state.powered = false;\n        state.state_id = 0;\n    }\n}\n";
        assert_eq!(source, expected);
    }

    #[test]
    fn timer_remaining_lowers_as_a_read_of_the_existing_timer_state_field() {
        let mut graph = charge_graph();
        graph.vars.push(VarDef {
            keep_across_form_switch: false,
            name: "remaining".into(),
            kind: VarType::Timer,
            initial: ConstValue::Int(30),
        });
        graph.transitions[0].guard = Predicate::Compare {
            op: CmpOp::Gt,
            left: Expr::TimerRemaining("remaining".into()),
            right: Expr::Const(ConstValue::Int(0)),
        };
        let source = emit("cloud", &graph, &charge_plan()).unwrap();
        assert!(source.contains("(state.remaining > 0)"), "{source}");
    }

    fn status_plan(symbol: &str, evidence: &str, runtime_validated: bool) -> EmissionPlan {
        EmissionPlan {
            bindings: vec![
                HookBinding {
                    hook: HookName::OnChargeTick,
                    article: None,
                    transitions: vec![0],
                },
                HookBinding {
                    hook: HookName::OnSpend,
                    article: None,
                    transitions: vec![1],
                },
            ],
            resource_triggers: Vec::new(),
            event_bindings: Vec::new(),
            status_triggers: vec![StatusTrigger {
                hook: HookName::OnSpend,
                symbol: symbol.to_string(),
                evidence: evidence.to_string(),
                runtime_validated,
                transitions: Vec::new(),
            }],
        }
    }

    #[test]
    fn exact_validated_status_binding_emits_deterministically() {
        // This is a contract fixture, not evidence that this status means a
        // Cloud spend in game.
        let plan = status_plan(
            "FIGHTER_CLOUD_STATUS_KIND_SPECIAL_LW_END",
            "synthetic contract fixture only",
            true,
        );
        let source = emit("cloud", &charge_graph(), &plan).unwrap();
        assert!(source.contains("gimmick_cloud_on_spend"));
        assert!(source.contains("StatusChange"));
        assert_eq!(source, emit("cloud", &charge_graph(), &plan).unwrap());
    }

    #[test]
    fn computed_flag_assignment_emits_deterministic_direct_state_write() {
        let mut graph = charge_graph();
        graph.transitions[0].actions = vec![Action::SetFlag {
            var: "powered".into(),
            value: Predicate::Compare {
                op: CmpOp::Gte,
                left: Expr::Var("gauge".into()),
                right: Expr::Const(ConstValue::Float(100.0)),
            },
        }];
        let plan = charge_plan();
        let source = emit("cloud", &graph, &plan).unwrap();
        assert!(
            source.contains("state.powered = (state.gauge >= 100.0);"),
            "{source}"
        );
        assert_eq!(source, emit("cloud", &graph, &plan).unwrap());
    }

    #[test]
    fn check_plan_typechecks_dynamic_values_in_the_exact_receiver_scope() {
        let (mut graph, plan) = typed_event_graph();
        graph.transitions[2].actions.push(Action::EmitEventValues {
            name: "ready".into(),
            arguments: vec![
                super::super::event_payload::ValueArgument {
                    field: "count".into(),
                    value: super::super::event_payload::EventValue::Numeric(Expr::EventField {
                        event: "ready".into(),
                        field: "count".into(),
                    }),
                },
                super::super::event_payload::ValueArgument {
                    field: "active".into(),
                    value: super::super::event_payload::EventValue::Flag(Predicate::EventFlag {
                        event: "ready".into(),
                        field: "active".into(),
                    }),
                },
            ],
        });
        assert!(check_plan(&graph, &plan).is_empty());
        assert!(emit("cloud", &graph, &plan)
            .unwrap_err()
            .iter()
            .any(|error| {
                error.item == "named events"
                    && error.reason.contains("cannot dispatch named events")
            }));

        graph.transitions[2].actions[0] = Action::EmitEventValues {
            name: "ready".into(),
            arguments: vec![
                super::super::event_payload::ValueArgument {
                    field: "active".into(),
                    value: super::super::event_payload::EventValue::Numeric(Expr::Const(
                        ConstValue::Int(1),
                    )),
                },
                super::super::event_payload::ValueArgument {
                    field: "count".into(),
                    value: super::super::event_payload::EventValue::Flag(Predicate::Flag(
                        "powered".into(),
                    )),
                },
            ],
        };
        let errors = check_plan(&graph, &plan);
        assert!(errors
            .iter()
            .any(|error| { error.reason.contains("expected Flag, received Int") }));
        assert!(errors
            .iter()
            .any(|error| { error.reason.contains("expected Int, received Flag") }));
    }

    #[test]
    fn colliding_names_get_distinct_valid_identifiers() {
        let mut graph = charge_graph();
        // Leading underscores strip, so this sanitizes to exactly "gauge".
        graph.vars.push(VarDef {
            keep_across_form_switch: false,
            name: "_gauge".to_string(),
            kind: VarType::Float,
            initial: ConstValue::Float(1.0),
        });
        let source = emit("cloud", &graph, &charge_plan()).unwrap();
        assert!(source.contains("pub gauge: f32,"), "{source}");
        assert!(source.contains("pub gauge_2: f32,"), "{source}");
        assert!(source.contains("gauge_2: 1.0,"), "{source}");
    }
}
