//! Constrained behavior IR (slice F2.1).
//!
//! A fighter gimmick like Cloud Limit is a small state machine over typed
//! variables: charge builds a float, a timer bounds the full state, a flag
//! marks the powered move, transitions fire on guarded predicates, and resets
//! (KO, timeout, spend) return variables to their initial values. This module
//! is that vocabulary and nothing else — no hook names (F2.2), no Rust
//! emission (F2.3), no plugin runtime (F2.4). Anything those slices need but
//! this IR cannot say is a follow-up slice, never an ad-hoc extension.
//!
//! [`validate`] rejects invalid graphs the way the params walker rejects bad
//! paths: every error names the item and the reason, and an empty graph (no
//! behavior authored) is valid. Persistence attaches to
//! [`FighterMod`](crate::mod_project::FighterMod) with serde defaults, so old
//! projects load unchanged.

// Hook mapping, source emission and runtime dispatch live in separate modules;
// this vocabulary remains independent of their engine-specific implementations.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use super::native_inputs::{CombatOp, NativeInput, NativeKind};
use super::registry::{self, IdentifierKind};

// The IR vocabulary is the live model's: the editor saves, validates and
// emits these exact types, and the plugin evaluates them, so nothing converts
// between a project graph and the live wire. Saved projects predate the shared
// names, hence the aliases.
pub use super::live_model::{
    Action, CmpOp, Expr, Operand, Predicate, Transition, Value as ConstValue, VarDef,
    VarKind as VarType,
};

impl fmt::Display for CmpOp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            CmpOp::Eq => "==",
            CmpOp::Lt => "<",
            CmpOp::Lte => "<=",
            CmpOp::Gt => ">",
            CmpOp::Gte => ">=",
        })
    }
}

/// One named controller state, optionally owning a fighter status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<super::status_model::StatusBinding>,
}

/// The whole behavior: entry state, named states, typed variables, guarded
/// transitions. An empty graph (no states, variables, or transitions and an
/// empty entry) means no behavior authored and validates clean.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BehaviorGraph {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub custom_statuses: BTreeMap<String, u8>,
    #[serde(default)]
    pub initial: String,
    #[serde(default)]
    pub states: Vec<StateDef>,
    #[serde(default)]
    pub vars: Vec<VarDef>,
    #[serde(default)]
    pub transitions: Vec<Transition>,
    /// Reviewed hook bindings travel with the graph. Without this the graph
    /// has no proven engine entry point, so adapter export refuses it.
    #[serde(
        default,
        skip_serializing_if = "super::codegen::EmissionPlan::is_empty"
    )]
    pub emission_plan: super::codegen::EmissionPlan,
}

impl BehaviorGraph {
    pub fn is_empty(&self) -> bool {
        self.custom_statuses.is_empty()
            && self.initial.is_empty()
            && self.states.is_empty()
            && self.vars.is_empty()
            && self.transitions.is_empty()
            && self.emission_plan.is_empty()
    }
}

/// One rejected item: what failed and why. Empty output from [`validate`]
/// means the graph is sound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BehaviorError {
    pub item: String,
    pub reason: String,
}

impl fmt::Display for BehaviorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "behavior {}: {}", self.item, self.reason)
    }
}

/// Reject graph shapes that would make the recursive validator, scope
/// resolver, or their diagnostics unbounded. This pass is iterative and
/// borrows the graph only; in particular, it runs before any user-controlled
/// label is interpolated into an error or any lookup map is built.
pub(crate) fn preflight_bounds(graph: &BehaviorGraph) -> Result<(), String> {
    use super::codegen::MAX_EVENT_BINDINGS;
    use super::event_payload::{MAX_FIELDS_PER_EVENT, MAX_FIELD_NAME_BYTES};
    use super::live_model::{
        MAX_ACTIONS_PER_TRANSITION, MAX_BINDINGS, MAX_EVENT_FIELDS, MAX_EXPR_DEPTH, MAX_EXPR_NODES,
        MAX_NAME_BYTES, MAX_NATIVE_INPUTS, MAX_STATES, MAX_SYMBOL_BYTES, MAX_TRANSITIONS,
        MAX_VARIABLES,
    };

    fn within(text: &str, limit: usize) -> bool {
        text.len() <= limit && !text.as_bytes().contains(&0)
    }

    if graph.states.len() > MAX_STATES {
        return Err(format!(
            "state count exceeds the runtime limit of {MAX_STATES}"
        ));
    }
    for state in &graph.states {
        if state.status.as_ref().is_some_and(|b| {
            !within(&b.target, MAX_SYMBOL_BYTES)
                || b.roots.values().any(|r| r.len() > MAX_TRANSITIONS)
        }) {
            return Err("status binding exceeds runtime bounds".into());
        }
    }
    if graph.emission_plan.event_bindings.len() > MAX_EVENT_BINDINGS {
        return Err(format!(
            "event bindings exceed the limit of {MAX_EVENT_BINDINGS}"
        ));
    }
    if graph.vars.len() > MAX_VARIABLES
        || graph.transitions.len() > MAX_TRANSITIONS
        || graph.emission_plan.bindings.len() > MAX_BINDINGS
        || graph.emission_plan.status_triggers.len() > MAX_BINDINGS
        || graph.emission_plan.resource_triggers.len() > MAX_BINDINGS
        || graph.emission_plan.event_bindings.len() > MAX_EVENT_BINDINGS
    {
        return Err("behavior graph exceeds a bounded table limit".into());
    }

    for binding in &graph.emission_plan.event_bindings {
        if binding.fields.len() > MAX_EVENT_FIELDS || binding.fields.len() > MAX_FIELDS_PER_EVENT {
            return Err(format!(
                "event payload has too many fields; maximum is {MAX_FIELDS_PER_EVENT}"
            ));
        }
        if binding.transitions.len() > MAX_TRANSITIONS {
            return Err(format!(
                "event transition list exceeds the limit of {MAX_TRANSITIONS}"
            ));
        }
    }

    if !within(&graph.initial, MAX_NAME_BYTES)
        || graph
            .states
            .iter()
            .any(|state| !within(&state.name, MAX_NAME_BYTES))
        || graph
            .vars
            .iter()
            .any(|var| !within(&var.name, MAX_NAME_BYTES))
        || graph.transitions.iter().any(|transition| {
            transition.actions.len() > MAX_ACTIONS_PER_TRANSITION
                || !within(&transition.from, MAX_NAME_BYTES)
                || !within(&transition.to, MAX_NAME_BYTES)
        })
        || graph
            .emission_plan
            .bindings
            .iter()
            .any(|binding| binding.transitions.len() > MAX_TRANSITIONS)
        || graph.emission_plan.status_triggers.iter().any(|trigger| {
            trigger.transitions.len() > MAX_TRANSITIONS
                || !within(&trigger.symbol, MAX_SYMBOL_BYTES)
                || !within(&trigger.evidence, 4_096)
        })
        || graph.emission_plan.event_bindings.iter().any(|binding| {
            binding.transitions.len() > MAX_TRANSITIONS
                || binding.fields.len() > MAX_EVENT_FIELDS
                || binding.fields.len() > MAX_FIELDS_PER_EVENT
                || !within(&binding.name, MAX_FIELD_NAME_BYTES)
                || binding
                    .fields
                    .iter()
                    .any(|field| !within(&field.name, MAX_FIELD_NAME_BYTES))
        })
    {
        return Err("behavior graph exceeds a bounded item limit".into());
    }

    // Native sources are deduplicated by identity at lowering time. Keep only
    // borrowed identities here so even a malformed graph cannot force clones.
    let mut native_inputs = BTreeSet::new();
    enum Node<'a> {
        Expr(&'a Expr, usize),
        Predicate(&'a Predicate, usize),
    }
    let mut remaining = MAX_EXPR_NODES;
    for transition in &graph.transitions {
        let mut pending = vec![Node::Predicate(&transition.guard, 1)];
        for action in &transition.actions {
            match action {
                Action::Status(command) => {
                    pending.extend(command.expressions().into_iter().map(|e| Node::Expr(e, 1)))
                }
                Action::AddSpeed { x, y } => {
                    pending.push(Node::Expr(x, 1));
                    pending.push(Node::Expr(y, 1));
                }
                Action::Object { symbol, op } => {
                    if !within(symbol, MAX_SYMBOL_BYTES) {
                        return Err("behavior graph contains an overlong symbol".into());
                    }
                    pending.extend(op.expressions().into_iter().map(|e| Node::Expr(e, 1)));
                }
                Action::SetNative { input, value } | Action::AddNative { input, value } => {
                    if !native_input_bounded(input) {
                        return Err("native write reference is too long".into());
                    }
                    pending.push(Node::Expr(value, 1));
                }
                Action::SetNativeFlag { input, value } => {
                    if !native_input_bounded(input) {
                        return Err("native write reference is too long".into());
                    }
                    pending.push(Node::Predicate(value, 1));
                }
                Action::Set { var, value } | Action::Add { var, value } => {
                    if !within(var, MAX_NAME_BYTES) {
                        return Err("behavior graph contains an overlong label".into());
                    }
                    pending.push(Node::Expr(value, 1));
                }
                Action::SetFlag { var, value } => {
                    if !within(var, MAX_NAME_BYTES) {
                        return Err("behavior graph contains an overlong label".into());
                    }
                    pending.push(Node::Predicate(value, 1));
                }
                Action::Raise { var } | Action::Clear { var } | Action::Reset { var } => {
                    if !within(var, MAX_NAME_BYTES) {
                        return Err("behavior graph contains an overlong label".into());
                    }
                }
                Action::GenerateArticle { symbol }
                | Action::RemoveArticle { symbol }
                | Action::RemoveNewestArticle { symbol }
                | Action::ArticleMovement { symbol, .. } => {
                    if !within(symbol, MAX_SYMBOL_BYTES) {
                        return Err("behavior graph contains an overlong symbol".into());
                    }
                }
                Action::RequestArticleStatus { symbol, status } => {
                    if !within(symbol, MAX_SYMBOL_BYTES) || !within(status, MAX_SYMBOL_BYTES) {
                        return Err("overlong article/status symbol".into());
                    }
                }
                Action::RequestFighterStatus { symbol }
                | Action::RequestWeaponStatus { symbol, .. } => {
                    if !within(symbol, MAX_SYMBOL_BYTES) {
                        return Err("behavior graph contains an overlong symbol".into());
                    }
                }
                Action::SetMotionRate { rate: amount }
                | Action::AddHeldOpponentDamage { amount }
                | Action::AddDamage { amount }
                | Action::CombatPolicy {
                    op:
                        CombatOp::DamageTaken(amount)
                        | CombatOp::KnockbackTaken(amount)
                        | CombatOp::DamageDealt(amount)
                        | CombatOp::KnockbackDealt(amount)
                        | CombatOp::Stat(_, amount)
                        | CombatOp::Armor(amount),
                    ..
                }
                | Action::Resource(super::resource_commands::Command { amount, .. }) => {
                    pending.push(Node::Expr(amount, 1))
                }
                Action::CombatPolicy { .. }
                | Action::ThrowHeldOpponent { .. }
                | Action::ReleaseHeldOpponent
                | Action::CancelIncomingHit
                | Action::Movement(_)
                | Action::Item(_)
                | Action::MeshVisibility { .. }
                | Action::ChooseHeroMenuSpell { .. } => {}
                Action::EmitEvent { name } => {
                    if !within(name, MAX_NAME_BYTES) {
                        return Err("behavior graph contains an overlong event name".into());
                    }
                }
                Action::EmitEventArguments { name, arguments } => {
                    if !within(name, MAX_NAME_BYTES)
                        || arguments.len() > MAX_EVENT_FIELDS
                        || arguments.len() > MAX_FIELDS_PER_EVENT
                    {
                        return Err("behavior graph exceeds a bounded event-argument limit".into());
                    }
                    if arguments
                        .iter()
                        .any(|argument| !within(&argument.field, MAX_FIELD_NAME_BYTES))
                    {
                        return Err("behavior graph contains an overlong event field name".into());
                    }
                }
                Action::EmitEventValues { name, arguments } => {
                    if !within(name, MAX_NAME_BYTES)
                        || arguments.len() > MAX_EVENT_FIELDS
                        || arguments.len() > MAX_FIELDS_PER_EVENT
                    {
                        return Err("behavior graph exceeds a bounded event-value limit".into());
                    }
                    if arguments
                        .iter()
                        .any(|argument| !within(&argument.field, MAX_FIELD_NAME_BYTES))
                    {
                        return Err("behavior graph contains an overlong event field name".into());
                    }
                    for argument in arguments {
                        match &argument.value {
                            super::event_payload::EventValue::Constant(_) => {}
                            super::event_payload::EventValue::Numeric(expr) => {
                                pending.push(Node::Expr(expr, 1));
                            }
                            super::event_payload::EventValue::Flag(predicate) => {
                                pending.push(Node::Predicate(predicate, 1));
                            }
                        }
                    }
                }
            }
        }

        while let Some(node) = pending.pop() {
            let depth = match node {
                Node::Expr(_, depth) | Node::Predicate(_, depth) => depth,
            };
            if depth > MAX_EXPR_DEPTH || remaining == 0 {
                return Err("behavior expression depth or node budget exceeded".into());
            }
            remaining -= 1;
            match node {
                Node::Expr(expr, depth) => match expr {
                    Expr::PendingDamage => {}
                    Expr::StatusFrame
                    | Expr::MotionFrame
                    | Expr::StickX { .. }
                    | Expr::StickY
                    | Expr::RandomInt { .. }
                    | Expr::RandomFloat { .. }
                    | Expr::Const(_) => {}
                    Expr::Var(name) => {
                        if !within(name, MAX_NAME_BYTES) {
                            return Err("behavior graph contains an overlong variable name".into());
                        }
                    }
                    Expr::TimerRemaining(name) => {
                        if !within(name, MAX_NAME_BYTES) {
                            return Err(
                                "behavior graph contains an overlong timer variable name".into()
                            );
                        }
                    }
                    Expr::EventField { event, field } => {
                        if !within(event, MAX_NAME_BYTES) || !within(field, MAX_FIELD_NAME_BYTES) {
                            return Err(
                                "behavior graph contains an overlong event field reference".into(),
                            );
                        }
                    }
                    Expr::NativeFloat(source) | Expr::NativeInt(source) => {
                        if !native_input_bounded(source) {
                            return Err(
                                "behavior graph contains an overlong native reference".into()
                            );
                        }
                        native_inputs.insert(source);
                        if native_inputs.len() > MAX_NATIVE_INPUTS {
                            return Err("behavior graph exceeds the native reference limit".into());
                        }
                    }
                    Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
                        pending.push(Node::Expr(right, depth + 1));
                        pending.push(Node::Expr(left, depth + 1));
                    }
                },
                Node::Predicate(predicate, depth) => match predicate {
                    Predicate::InStatus { symbol } | Predicate::TermEnabled { symbol } => {
                        if !within(symbol, MAX_SYMBOL_BYTES) {
                            return Err("overlong status symbol".into());
                        }
                    }
                    Predicate::InMotion { motion } => {
                        if !super::native_inputs::motion_name_valid(motion) {
                            return Err("invalid motion name".into());
                        }
                    }
                    Predicate::DamageIsProjectile
                    | Predicate::SituationChanged
                    | Predicate::MotionEnded
                    | Predicate::CancelEnabled
                    | Predicate::Situation { .. }
                    | Predicate::Button { .. } => {}
                    Predicate::Flag(name) | Predicate::Expired(name) => {
                        if !within(name, MAX_NAME_BYTES) {
                            return Err("behavior graph contains an overlong variable name".into());
                        }
                    }
                    Predicate::EventFlag { event, field } => {
                        if !within(event, MAX_NAME_BYTES) || !within(field, MAX_FIELD_NAME_BYTES) {
                            return Err(
                                "behavior graph contains an overlong event field reference".into(),
                            );
                        }
                    }
                    Predicate::DamageStatusIs(source) => {
                        if !native_input_bounded(source) {
                            return Err(
                                "behavior graph contains an overlong contact status selector"
                                    .into(),
                            );
                        }
                    }
                    Predicate::NativeFlag(source) => {
                        if !native_input_bounded(source) {
                            return Err(
                                "behavior graph contains an overlong native reference".into()
                            );
                        }
                        native_inputs.insert(source);
                        if native_inputs.len() > MAX_NATIVE_INPUTS {
                            return Err("behavior graph exceeds the native reference limit".into());
                        }
                    }
                    Predicate::Compare { left, right, .. } => {
                        pending.push(Node::Expr(right, depth + 1));
                        pending.push(Node::Expr(left, depth + 1));
                    }
                    Predicate::Not(inner) => pending.push(Node::Predicate(inner, depth + 1)),
                    Predicate::And(left, right) | Predicate::Or(left, right) => {
                        pending.push(Node::Predicate(right, depth + 1));
                        pending.push(Node::Predicate(left, depth + 1));
                    }
                },
            }
        }
    }

    if native_inputs.len() > MAX_NATIVE_INPUTS {
        return Err("behavior graph exceeds the native reference limit".into());
    }
    Ok(())
}

fn native_input_bounded(source: &NativeInput) -> bool {
    source.reference_is_bounded()
}

/// Receiver scopes for event-field reads. The index borrows declarations from
/// the graph, is built only after all table and transition-list bounds pass,
/// and never inserts event fields into the global variable namespace.
pub(crate) struct EventReadScopes<'a> {
    graph: &'a BehaviorGraph,
    by_transition: Vec<Vec<&'a super::codegen::EventBinding>>,
    by_name: BTreeMap<&'a str, Option<&'a super::codegen::EventBinding>>,
    conflicted_transitions: Vec<bool>,
}

impl<'a> EventReadScopes<'a> {
    pub(crate) fn build(graph: &'a BehaviorGraph, errors: &mut Vec<BehaviorError>) -> Self {
        use super::codegen::MAX_EVENT_BINDINGS;
        use super::live_model::{MAX_BINDINGS, MAX_TRANSITIONS};

        let transition_count = graph.transitions.len();
        if transition_count > MAX_TRANSITIONS {
            errors.push(BehaviorError {
                item: "event field scopes".into(),
                reason: format!(
                    "graph has {transition_count} transitions; event field scope limit is {MAX_TRANSITIONS}"
                ),
            });
            return Self {
                graph,
                by_transition: Vec::new(),
                by_name: BTreeMap::new(),
                conflicted_transitions: Vec::new(),
            };
        }

        let mut by_transition = vec![Vec::new(); transition_count];
        // Resource-trigger ownership is represented by its hook binding in
        // this model. `on_frame` is an instant-cycle boundary, not an
        // independent dispatcher owner.
        let mut conflicted_transitions = vec![false; transition_count];

        // A hook or a status-triggered hook supplies a different runtime
        // context. If those ownership lists cannot be bounded, no transition
        // can be proven to have an exclusive event context.
        let mut hook_ownership_bounded = graph.emission_plan.bindings.len() <= MAX_BINDINGS;
        if !hook_ownership_bounded {
            errors.push(BehaviorError {
                item: "event field scopes".into(),
                reason: format!("hook binding table exceeds the limit of {MAX_BINDINGS}"),
            });
            conflicted_transitions.fill(true);
        } else {
            for (binding_index, binding) in graph.emission_plan.bindings.iter().enumerate() {
                if binding.transitions.len() > MAX_TRANSITIONS {
                    hook_ownership_bounded = false;
                    errors.push(BehaviorError {
                        item: format!("hook binding {binding_index}"),
                        reason: format!("transition list exceeds the limit of {MAX_TRANSITIONS}"),
                    });
                    conflicted_transitions.fill(true);
                    break;
                }
                for transition in &binding.transitions {
                    if let Some(conflict) = conflicted_transitions.get_mut(*transition) {
                        *conflict = true;
                    }
                }
            }
        }
        if graph.emission_plan.status_triggers.len() > MAX_BINDINGS {
            errors.push(BehaviorError {
                item: "event field scopes".into(),
                reason: format!("status trigger table exceeds the limit of {MAX_BINDINGS}"),
            });
            conflicted_transitions.fill(true);
        } else {
            for (trigger_index, trigger) in graph.emission_plan.status_triggers.iter().enumerate() {
                if trigger.transitions.len() > MAX_TRANSITIONS {
                    errors.push(BehaviorError {
                        item: format!("status trigger {trigger_index}"),
                        reason: format!(
                            "transition selection exceeds the limit of {MAX_TRANSITIONS}"
                        ),
                    });
                    conflicted_transitions.fill(true);
                    continue;
                }
                for transition in &trigger.transitions {
                    if let Some(conflict) = conflicted_transitions.get_mut(*transition) {
                        *conflict = true;
                    }
                }
            }
        }

        if graph.emission_plan.event_bindings.len() > MAX_EVENT_BINDINGS {
            errors.push(BehaviorError {
                item: "event bindings".into(),
                reason: format!("exceeds the limit of {MAX_EVENT_BINDINGS}"),
            });
            return Self {
                graph,
                by_transition,
                by_name: BTreeMap::new(),
                conflicted_transitions,
            };
        }

        let mut counts = BTreeMap::<&str, usize>::new();
        for binding in &graph.emission_plan.event_bindings {
            if !binding.name.trim().is_empty()
                && binding.name.len() <= super::live_model::MAX_NAME_BYTES
                && !binding.name.contains('\0')
            {
                *counts.entry(binding.name.as_str()).or_default() += 1;
            }
        }
        let mut by_name = BTreeMap::new();
        for (binding_index, binding) in graph.emission_plan.event_bindings.iter().enumerate() {
            let item = format!("event binding {binding_index}");
            let name_valid = !binding.name.trim().is_empty()
                && binding.name.len() <= super::live_model::MAX_NAME_BYTES
                && !binding.name.contains('\0');
            if !name_valid {
                errors.push(BehaviorError {
                    item: item.clone(),
                    reason: format!(
                        "event name must be non-empty, NUL-free, and at most {} bytes",
                        super::live_model::MAX_NAME_BYTES
                    ),
                });
            }
            let schema_valid = match super::event_payload::validate_schema(
                "behavior",
                &binding.name,
                &binding.fields,
            ) {
                Ok(()) => true,
                Err(schema_errors) => {
                    errors.extend(schema_errors.into_iter().map(|error| BehaviorError {
                        item: item.clone(),
                        reason: error.to_string(),
                    }));
                    false
                }
            };
            let transitions_bounded = binding.transitions.len() <= MAX_TRANSITIONS;
            if !transitions_bounded {
                errors.push(BehaviorError {
                    item: item.clone(),
                    reason: format!("event transition list exceeds the limit of {MAX_TRANSITIONS}"),
                });
            }
            let unique_name = name_valid
                && counts
                    .get(binding.name.as_str())
                    .copied()
                    .unwrap_or_default()
                    == 1;
            if name_valid && !unique_name {
                errors.push(BehaviorError {
                    item: item.clone(),
                    reason: "event name has more than one receiver binding".into(),
                });
            }
            if name_valid {
                by_name.insert(
                    binding.name.as_str(),
                    (unique_name && schema_valid).then_some(binding),
                );
            }

            if !unique_name || !schema_valid || !transitions_bounded || !hook_ownership_bounded {
                continue;
            }
            let mut seen = BTreeSet::new();
            let mut duplicate_transition = false;
            for transition in &binding.transitions {
                if !seen.insert(*transition) {
                    duplicate_transition = true;
                    errors.push(BehaviorError {
                        item: item.clone(),
                        reason: format!("transition {transition} is listed more than once"),
                    });
                }
                if *transition >= transition_count {
                    errors.push(BehaviorError {
                        item: item.clone(),
                        reason: format!("transition index {transition} is outside the graph"),
                    });
                }
            }
            if duplicate_transition {
                continue;
            }
            for transition in &binding.transitions {
                if let Some(owners) = by_transition.get_mut(*transition) {
                    owners.push(binding);
                }
            }
        }
        Self {
            graph,
            by_transition,
            by_name,
            conflicted_transitions,
        }
    }

    fn resolve(
        &self,
        transition: usize,
        event: &str,
        field: &str,
    ) -> Result<VarType, &'static str> {
        let valid_name = |name: &str| {
            !name.trim().is_empty()
                && name.len() <= super::live_model::MAX_NAME_BYTES
                && !name.contains('\0')
        };
        if !valid_name(event) || !valid_name(field) {
            return Err("event and field names must be non-empty, NUL-free, and within 48 bytes");
        }
        let binding = match self.by_name.get(event).copied() {
            Some(Some(binding)) => binding,
            Some(None) => return Err("event name has no unique valid receiver schema"),
            None => return Err("event field read names an unknown receiver event"),
        };
        if self
            .conflicted_transitions
            .get(transition)
            .copied()
            .unwrap_or(true)
        {
            return Err(
                "transition has a conflicting hook or status owner, not an exclusive event context",
            );
        }
        let owners = self
            .by_transition
            .get(transition)
            .ok_or("transition has no event receiver context")?;
        if owners.len() != 1 || owners[0].name != event {
            return Err("event field read is outside its exact receiver transition scope");
        }
        binding
            .fields
            .iter()
            .find(|candidate| candidate.name == field)
            .map(|candidate| candidate.kind)
            .ok_or("event field read names an unknown receiver field")
    }

    pub(crate) fn checked(graph: &'a BehaviorGraph) -> Result<Self, String> {
        let mut errors = Vec::new();
        let scopes = Self::build(graph, &mut errors);
        if errors.is_empty() {
            Ok(scopes)
        } else {
            Err(errors
                .into_iter()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; "))
        }
    }

    pub(crate) fn field_kind(
        &self,
        transition: usize,
        event: &str,
        field: &str,
    ) -> Result<VarType, String> {
        self.resolve(transition, event, field)
            .map_err(str::to_owned)
    }

    pub(crate) fn pending_damage_allowed(&self, transition: usize) -> bool {
        let plan = &self.graph.emission_plan;
        let owners: Vec<_> = plan
            .bindings
            .iter()
            .filter(|binding| binding.transitions.contains(&transition))
            .map(|binding| binding.hook)
            .collect();
        !owners.is_empty()
            && owners.iter().all(|hook| {
                matches!(
                    hook,
                    super::hooks::HookName::OnDealDamage
                        | super::hooks::HookName::OnTakeDamage
                        | super::hooks::HookName::OnAttackShielded
                        | super::hooks::HookName::OnShieldHit
                        | super::hooks::HookName::OnIncomingHit
                        | super::hooks::HookName::OnReflect
                        | super::hooks::HookName::OnAbsorb
                )
            })
            && !plan
                .event_bindings
                .iter()
                .any(|binding| binding.transitions.contains(&transition))
            && !plan.status_triggers.iter().any(|trigger| {
                trigger.transitions.contains(&transition) || owners.contains(&trigger.hook)
            })
            && !plan
                .resource_triggers
                .iter()
                .any(|trigger| owners.contains(&trigger.hook))
    }
}

/// Check a graph and collect every problem. The checks run in a fixed order
/// — shape, references, types, cycles — so a graph with several failures
/// reports all of them, mirroring how the params walker reports each bad
/// edit rather than stopping at the first.
pub fn validate(graph: &BehaviorGraph) -> Vec<BehaviorError> {
    let mut errors = Vec::new();
    if let Err(reason) = preflight_bounds(graph) {
        errors.push(BehaviorError {
            item: "graph bounds".into(),
            reason,
        });
        return errors;
    }
    if graph.is_empty() {
        return errors;
    }

    let mut states: BTreeSet<&str> = BTreeSet::new();
    for state in &graph.states {
        if state.name.is_empty() {
            errors.push(BehaviorError {
                item: "state".to_string(),
                reason: "a state with an empty name names nothing".to_string(),
            });
            continue;
        }
        if !states.insert(state.name.as_str()) {
            errors.push(BehaviorError {
                item: format!("state {:?}", state.name),
                reason: "the state is declared more than once".to_string(),
            });
        }
    }

    let mut vars: BTreeMap<&str, &VarDef> = BTreeMap::new();
    for var in &graph.vars {
        if var.name.is_empty() {
            errors.push(BehaviorError {
                item: "var".to_string(),
                reason: "a variable with an empty name names nothing".to_string(),
            });
            continue;
        }
        if vars.insert(var.name.as_str(), var).is_some() {
            errors.push(BehaviorError {
                item: format!("var {:?}", var.name),
                reason: "the variable is declared more than once".to_string(),
            });
        }
        let declared = var.kind;
        let given = match (&var.kind, &var.initial) {
            (VarType::Timer, ConstValue::Int(_)) => VarType::Timer,
            (_, initial) => initial.kind(),
        };
        if given != declared {
            errors.push(BehaviorError {
                item: format!("var {:?}", var.name),
                reason: format!(
                    "declared {:?} but initialized as {:?}; a timer starts from a frame count",
                    declared, given
                ),
            });
        }
        match var.initial {
            ConstValue::Float(value) if !value.is_finite() => errors.push(BehaviorError {
                item: format!("var {:?}", var.name),
                reason: "a float initializer must be finite".to_string(),
            }),
            ConstValue::Float(value)
                if f64::from(value).abs() > super::live_model::MAX_ABS_NUMBER =>
            {
                errors.push(BehaviorError {
                    item: format!("var {:?}", var.name),
                    reason: "a numeric initializer is outside the bounded range".to_string(),
                })
            }
            ConstValue::Int(value)
                if i64::from(value).abs() > super::live_model::MAX_ABS_NUMBER as i64 =>
            {
                errors.push(BehaviorError {
                    item: format!("var {:?}", var.name),
                    reason: "an integer initializer is outside the bounded range".to_string(),
                })
            }
            ConstValue::Float(_) | ConstValue::Int(_) | ConstValue::Flag(_) => {}
        }
    }

    if graph.initial.is_empty() {
        errors.push(BehaviorError {
            item: "graph".to_string(),
            reason: "a non-empty graph needs an entry state".to_string(),
        });
    } else if !states.contains(graph.initial.as_str()) {
        errors.push(BehaviorError {
            item: format!("entry {:?}", graph.initial),
            reason: "the entry state is not a declared state".to_string(),
        });
    }

    if let Err(reason) = super::status_model::validate(
        None,
        &graph.initial,
        graph
            .states
            .iter()
            .filter_map(|s| s.status.as_ref().map(|b| (s.name.as_str(), b))),
        &graph
            .transitions
            .iter()
            .map(|t| (t.from.as_str(), t.to.as_str()))
            .collect::<Vec<_>>(),
        &graph
            .emission_plan
            .bindings
            .iter()
            .flat_map(|b| b.transitions.iter().copied())
            .chain(
                graph
                    .emission_plan
                    .event_bindings
                    .iter()
                    .flat_map(|b| b.transitions.iter().copied()),
            )
            .chain(
                graph
                    .emission_plan
                    .status_triggers
                    .iter()
                    .flat_map(|b| b.transitions.iter().copied()),
            )
            .collect(),
    ) {
        errors.push(BehaviorError {
            item: "status bindings".into(),
            reason,
        });
    }
    let event_scopes = EventReadScopes::build(graph, &mut errors);

    for (index, transition) in graph.transitions.iter().enumerate() {
        let item = format!(
            "transition {} {:?} -> {:?}",
            index, transition.from, transition.to
        );
        for (side, name) in [("from", &transition.from), ("to", &transition.to)] {
            if !states.contains(name.as_str()) {
                errors.push(BehaviorError {
                    item: item.clone(),
                    reason: format!("{side} names {name:?}, which is not a declared state"),
                });
            }
        }
        let status_phase = super::status_model::phase(
            graph.states.iter().filter_map(|s| s.status.as_ref()),
            index,
        );
        if status_phase.is_none() && transition_uses_reads(transition, false, false, true, false) {
            errors.push(BehaviorError {
                item: item.clone(),
                reason: "cancel observations require an installed status phase root".into(),
            });
        }
        if status_phase == Some(super::status_model::Phase::Pre)
            && transition_uses_input(transition)
        {
            errors.push(BehaviorError {
                item: item.clone(),
                reason: "Pre cannot read controller input or motion/status phase observations"
                    .into(),
            });
        }
        if transition_uses_reads(transition, false, false, false, true)
            && (status_phase.is_some()
                || !transition.on_frame
                || graph
                    .emission_plan
                    .bindings
                    .iter()
                    .filter(|b| b.transitions.contains(&index))
                    .any(|b| b.hook != super::hooks::HookName::OnChargeTick))
        {
            errors.push(BehaviorError {
                item: item.clone(),
                reason: "command inputs require a Main frame tick".into(),
            });
        }
        if transition_uses_input(transition) && status_phase.is_none() {
            let owners: Vec<_> = graph
                .emission_plan
                .bindings
                .iter()
                .filter(|b| b.transitions.contains(&index))
                .collect();
            if (!transition.on_frame
                && transition_uses_phase(transition)
                && !owners
                    .iter()
                    .all(|b| b.hook == super::hooks::HookName::OnIncomingHit))
                || owners.is_empty()
                || owners.iter().any(|b| {
                    b.hook != super::hooks::HookName::OnChargeTick
                        && b.hook != super::hooks::HookName::OnIncomingHit
                        && (!matches!(
                            b.hook,
                            super::hooks::HookName::OnItemPickup
                                | super::hooks::HookName::OnItemThrown
                                | super::hooks::HookName::OnGrab
                                | super::hooks::HookName::OnThrow
                                | super::hooks::HookName::OnGrabbed
                        ) || transition_uses_reads(transition, false, false, false, false))
                })
                || graph
                    .emission_plan
                    .event_bindings
                    .iter()
                    .any(|b| b.transitions.contains(&index))
                || graph
                    .emission_plan
                    .status_triggers
                    .iter()
                    .any(|b| b.transitions.contains(&index))
            {
                errors.push(BehaviorError {
                    item: item.clone(),
                    reason:
                        "controller input requires an exclusive onChargeTick frame-tick binding; phase reads also allow a grab/throw event boundary"
                            .into(),
                });
            }
        }
        check_predicate(
            &transition.guard,
            &vars,
            &event_scopes,
            index,
            &item,
            &mut errors,
        );
        if transition
            .actions
            .iter()
            .any(|a| matches!(a, Action::CancelIncomingHit))
        {
            let owners: Vec<_> = graph
                .emission_plan
                .bindings
                .iter()
                .filter(|b| b.transitions.contains(&index))
                .collect();
            if !event_scopes.pending_damage_allowed(index)
                || owners
                    .iter()
                    .any(|b| b.hook != super::hooks::HookName::OnIncomingHit)
            {
                errors.push(BehaviorError {
                    item: item.clone(),
                    reason: "CancelIncomingHit requires an exclusive onIncomingHit root".into(),
                });
            }
        }
        for (action_index, action) in transition.actions.iter().enumerate() {
            if let Some(phase) = status_phase {
                if super::live_protocol::convert_action(
                    action,
                    &graph.emission_plan.event_bindings,
                    graph,
                    index,
                )
                .is_ok_and(|a| !super::status_model::permits(phase, &a))
                {
                    errors.push(BehaviorError {
                        item: item.clone(),
                        reason: format!("action is not permitted in status {phase:?}"),
                    });
                }
            }
            check_action(
                action,
                &vars,
                &event_scopes,
                index,
                &format!("{item} / action {action_index}"),
                &mut errors,
            );
        }
    }

    check_instant_cycles(graph, &states, &mut errors);
    errors
}

/// Validate a graph in one exact fighter namespace.
///
/// Structural validation deliberately remains fighter-independent so old
/// projects can load without a roster lookup. Adapter export calls this
/// companion gate before rendering: engine actions must resolve to an exact
/// allowlisted entry for `fighter`, never a raw id or a symbol borrowed from
/// another fighter. Weapon-status actions additionally require a generated
/// concrete weapon-kind guard. Every diagnostic names the transition and
/// action index.
pub fn validate_for_fighter(fighter: &str, graph: &BehaviorGraph) -> Vec<BehaviorError> {
    let mut errors = validate(graph);
    if let Err(reason) = super::status_model::validate_allocations(fighter, &graph.custom_statuses)
    {
        errors.push(BehaviorError {
            item: "custom statuses".into(),
            reason,
        });
    }
    for binding in graph.states.iter().filter_map(|s| s.status.as_ref()) {
        if let Err(reason) = binding.target.in_manifest(&graph.custom_statuses) {
            errors.push(BehaviorError {
                item: "status binding".into(),
                reason,
            });
        }
    }
    if preflight_bounds(graph).is_err() {
        return errors;
    }
    if let Err(reason) = super::status_model::validate(
        Some(fighter),
        &graph.initial,
        graph
            .states
            .iter()
            .filter_map(|s| s.status.as_ref().map(|b| (s.name.as_str(), b))),
        &graph
            .transitions
            .iter()
            .map(|t| (t.from.as_str(), t.to.as_str()))
            .collect::<Vec<_>>(),
        &BTreeSet::new(),
    ) {
        errors.push(BehaviorError {
            item: "status bindings".into(),
            reason,
        });
    }
    for (transition_index, transition) in graph.transitions.iter().enumerate() {
        check_native_predicate_fighter(
            &transition.guard,
            fighter,
            &format!("transition {transition_index} guard"),
            &mut errors,
        );
        for (action_index, action) in transition.actions.iter().enumerate() {
            let action_item = format!("transition {transition_index} / action {action_index}");
            if let Action::Resource(command) = action {
                if let Err(reason) = command.check_owner(fighter) {
                    errors.push(BehaviorError {
                        item: action_item.clone(),
                        reason,
                    });
                }
            }
            if let Action::ChooseHeroMenuSpell { slot } = action {
                if let Err(reason) =
                    (super::hero_menu_commands::Command::Choose { slot: *slot }).validate(fighter)
                {
                    errors.push(BehaviorError {
                        item: action_item.clone(),
                        reason,
                    });
                }
            }
            if let Action::SetNative { input, .. }
            | Action::AddNative { input, .. }
            | Action::SetNativeFlag { input, .. } = action
            {
                let value = match action {
                    Action::SetNative {
                        value: Expr::Const(ConstValue::Int(value)),
                        ..
                    } => Some(*value),
                    _ => None,
                };
                if let Err(reason) = input
                    .validate_write(fighter)
                    .and_then(|()| value.map_or(Ok(()), |value| input.validate_write_int(value)))
                {
                    errors.push(BehaviorError {
                        item: action_item.clone(),
                        reason,
                    });
                }
            }
            // Status command arguments have never been fighter-checked here; kept as found.
            for operand in if matches!(action, Action::Status(_)) {
                Vec::new()
            } else {
                action.operands()
            } {
                match operand {
                    Operand::Expr(expr) => {
                        check_native_expr_fighter(expr, fighter, &action_item, &mut errors)
                    }
                    Operand::Predicate(predicate) => check_native_predicate_fighter(
                        predicate,
                        fighter,
                        &action_item,
                        &mut errors,
                    ),
                }
            }
            if let Action::RequestArticleStatus { symbol, status } = action {
                for (symbol, kind) in [
                    (symbol, IdentifierKind::Article),
                    (status, IdentifierKind::WeaponStatus),
                ] {
                    if let Err(reason) = registry::resolve(fighter, symbol, kind) {
                        errors.push(BehaviorError {
                            item: action_item.clone(),
                            reason: reason.to_string(),
                        });
                    }
                }
            }
            let (operation, symbol, kind) = match action {
                Action::GenerateArticle { symbol } => {
                    ("generate article", symbol, IdentifierKind::Article)
                }
                Action::RemoveArticle { symbol }
                | Action::RemoveNewestArticle { symbol }
                | Action::ArticleMovement { symbol, .. }
                | Action::Object { symbol, .. } => {
                    ("remove article", symbol, IdentifierKind::Article)
                }
                Action::RequestFighterStatus { symbol } => (
                    "request fighter status",
                    symbol,
                    IdentifierKind::FighterStatus,
                ),
                Action::RequestWeaponStatus { symbol, .. } => (
                    "request weapon status",
                    symbol,
                    IdentifierKind::WeaponStatus,
                ),
                _ => continue,
            };
            let item = action_item;
            match registry::resolve(fighter, symbol, kind) {
                Ok(entry)
                    if kind != IdentifierKind::WeaponStatus
                        || registry::weapon_kind(entry.fighter, entry.group).is_some() => {}
                Ok(entry) => errors.push(BehaviorError {
                    item,
                    reason: format!(
                        "{operation} requires a generated weapon-kind guard for {}/{}",
                        entry.fighter, entry.group
                    ),
                }),
                Err(error) => errors.push(BehaviorError {
                    item,
                    reason: format!(
                        "{operation} requires an allowlisted {kind:?} symbol: {error:?}"
                    ),
                }),
            }
        }
    }
    errors
}

/// Return the graph's deduplicated native-source declaration list in stable
/// enum order. Static/live compilers emit this once per program; snapshots are
/// then sampled once regardless of reference count.
pub fn collect_native_inputs(graph: &BehaviorGraph) -> Vec<NativeInput> {
    // This public helper is also used on deserialized graphs. Invalid input is
    // handled by `validate`; do not recursively walk it a second time here.
    if preflight_bounds(graph).is_err() {
        return Vec::new();
    }
    let mut inputs = BTreeSet::new();
    for transition in &graph.transitions {
        collect_predicate_inputs(&transition.guard, &mut inputs);
        for operand in transition.actions.iter().flat_map(Action::operands) {
            match operand {
                Operand::Expr(expr) => collect_expr_inputs(expr, &mut inputs),
                Operand::Predicate(predicate) => collect_predicate_inputs(predicate, &mut inputs),
            }
        }
    }
    inputs.into_iter().collect()
}

fn collect_expr_inputs(expr: &Expr, inputs: &mut BTreeSet<NativeInput>) {
    match expr {
        Expr::NativeFloat(source) => {
            inputs.insert(source.canonical());
        }
        Expr::NativeInt(source) => {
            inputs.insert(source.canonical());
        }
        Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
            collect_expr_inputs(left, inputs);
            collect_expr_inputs(right, inputs);
        }
        Expr::PendingDamage
        | Expr::StatusFrame
        | Expr::MotionFrame
        | Expr::StickX { .. }
        | Expr::StickY
        | Expr::RandomInt { .. }
        | Expr::RandomFloat { .. }
        | Expr::Const(_)
        | Expr::Var(_)
        | Expr::TimerRemaining(_)
        | Expr::EventField { .. } => {}
    }
}

/// Contact descriptors belong to root payloads, not the sampled native-input
/// declaration. Keep feature and ownership inspection separate from sampling.
pub(crate) fn collect_damage_status_selectors(graph: &BehaviorGraph) -> Vec<&NativeInput> {
    if preflight_bounds(graph).is_err() {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut pending = Vec::new();
    for transition in &graph.transitions {
        pending.push(&transition.guard);
        for action in &transition.actions {
            match action {
                Action::SetNativeFlag { value, .. } | Action::SetFlag { value, .. } => {
                    pending.push(value)
                }
                Action::EmitEventValues { arguments, .. } => {
                    for argument in arguments {
                        if let super::event_payload::EventValue::Flag(value) = &argument.value {
                            pending.push(value);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    while let Some(predicate) = pending.pop() {
        match predicate {
            Predicate::DamageStatusIs(source) => result.push(source),
            Predicate::Not(inner) => pending.push(inner),
            Predicate::And(left, right) | Predicate::Or(left, right) => {
                pending.push(left);
                pending.push(right);
            }
            _ => {}
        }
    }
    result
}

pub(crate) fn required_native_features(graph: &BehaviorGraph) -> BTreeSet<&'static str> {
    let mut result = collect_native_inputs(graph)
        .iter()
        .filter_map(NativeInput::required_feature)
        .collect::<BTreeSet<_>>();
    if graph.states.iter().any(|s| s.status.is_some()) {
        result.insert(super::status_model::FEATURE);
    }
    if !graph.custom_statuses.is_empty()
        || graph.states.iter().any(|s| s.status.is_some())
        || graph
            .transitions
            .iter()
            .any(|t| t.actions.iter().any(|a| matches!(a, Action::Status(_))))
    {
        result.insert(super::status_model::CANCEL_FEATURE);
    }
    if graph.emission_plan.bindings.iter().any(|b| {
        matches!(
            b.hook,
            super::hooks::HookName::OnGrab
                | super::hooks::HookName::OnThrow
                | super::hooks::HookName::OnGrabbed
        )
    }) {
        result.insert(super::native_inputs::GRAB_EVENTS_FEATURE);
    }
    if graph.emission_plan.bindings.iter().any(|b| {
        matches!(
            b.hook,
            super::hooks::HookName::OnItemPickup | super::hooks::HookName::OnItemThrown
        )
    }) {
        result.insert(super::native_inputs::ITEM_EVENTS_FEATURE);
    }
    if graph.emission_plan.bindings.iter().any(|b| {
        matches!(
            b.hook,
            super::hooks::HookName::OnReflect | super::hooks::HookName::OnAbsorb
        )
    }) {
        result.insert(super::native_inputs::REFLECT_ABSORB_FEATURE);
    }
    if graph
        .emission_plan
        .bindings
        .iter()
        .any(|b| b.hook == super::hooks::HookName::OnIncomingHit)
    {
        result.insert(super::native_inputs::INCOMING_HIT_FEATURE);
    }
    if graph.transitions.iter().any(transition_uses_phase) {
        result.insert(super::native_inputs::STATUS_PHASE_FEATURE);
    }
    if graph.transitions.iter().flat_map(|t| &t.actions).any(|a| {
        matches!(
            a,
            Action::AddHeldOpponentDamage { .. }
                | Action::ThrowHeldOpponent { .. }
                | Action::ReleaseHeldOpponent
        )
    }) {
        result.insert(super::native_inputs::HELD_OPPONENT_FEATURE);
    }
    if graph
        .transitions
        .iter()
        .flat_map(|t| &t.actions)
        .any(|a| matches!(a, Action::Resource(_)))
    {
        result.insert(super::resource_commands::FEATURE);
    }
    if graph
        .transitions
        .iter()
        .flat_map(|t| &t.actions)
        .any(|a| matches!(a, Action::ChooseHeroMenuSpell { .. }))
    {
        result.insert(super::hero_menu_commands::FEATURE);
    }
    if graph
        .transitions
        .iter()
        .flat_map(|t| &t.actions)
        .any(|a| matches!(a, Action::Movement(_) | Action::MeshVisibility { .. }))
    {
        result.insert(super::native_inputs::MOVEMENT_FEATURE);
    }
    if graph
        .transitions
        .iter()
        .flat_map(|t| &t.actions)
        .any(|a| matches!(a, Action::Item(_)))
    {
        result.insert(super::native_inputs::ITEMS_FEATURE);
    }
    if !collect_damage_status_selectors(graph).is_empty() {
        result.insert(super::native_inputs::DAMAGE_CAPTURE_STATUS_FEATURE);
    }
    if graph
        .transitions
        .iter()
        .take(super::live_model::MAX_TRANSITIONS)
        .flat_map(|transition| {
            transition
                .actions
                .iter()
                .take(super::live_model::MAX_ACTIONS_PER_TRANSITION)
        })
        .any(|action| {
            matches!(action, Action::Resource(command) if command.resource.entry().writer == super::resource_commands::Writer::CloudLimit)
        })
    {
        result.insert(super::native_inputs::CLOUD_LIMIT_ACTIONS_FEATURE);
    }
    if graph.emission_plan.bindings.iter().any(|binding| {
        binding.article.is_some() || binding.hook == super::hooks::HookName::OnArticleRemoved
    }) || graph.transitions.iter().any(|transition| {
        transition.actions.iter().any(|action| {
            matches!(
                action,
                Action::RequestArticleStatus { .. }
                    | Action::RemoveNewestArticle { .. }
                    | Action::ArticleMovement { .. }
                    | Action::Object { .. }
            )
        })
    }) {
        result.insert(super::native_inputs::OWNED_ARTICLES_FEATURE);
    }
    result
}

fn collect_predicate_inputs(predicate: &Predicate, inputs: &mut BTreeSet<NativeInput>) {
    match predicate {
        Predicate::Compare { left, right, .. } => {
            collect_expr_inputs(left, inputs);
            collect_expr_inputs(right, inputs);
        }
        Predicate::NativeFlag(source) => {
            inputs.insert(source.canonical());
        }
        Predicate::Not(inner) => collect_predicate_inputs(inner, inputs),
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            collect_predicate_inputs(left, inputs);
            collect_predicate_inputs(right, inputs);
        }
        Predicate::DamageIsProjectile
        | Predicate::InStatus { .. }
        | Predicate::InMotion { .. }
        | Predicate::SituationChanged
        | Predicate::MotionEnded
        | Predicate::CancelEnabled
        | Predicate::TermEnabled { .. }
        | Predicate::Situation { .. }
        | Predicate::Button { .. }
        | Predicate::Flag(_)
        | Predicate::EventFlag { .. }
        | Predicate::Expired(_)
        | Predicate::DamageStatusIs(_) => {}
    }
}

fn check_native_expr_fighter(
    expr: &Expr,
    fighter: &str,
    item: &str,
    errors: &mut Vec<BehaviorError>,
) {
    match expr {
        Expr::NativeFloat(source)
            if !source.valid_for(fighter) || source.kind() != Some(NativeKind::Float) =>
        {
            errors.push(BehaviorError {
                item: item.to_string(),
                reason: format!(
                    "native input {:?} has the wrong owner or type for fighter {fighter:?}",
                    source
                ),
            })
        }
        Expr::NativeInt(source)
            if !source.valid_for(fighter) || source.kind() != Some(NativeKind::Int) =>
        {
            errors.push(BehaviorError {
                item: item.to_string(),
                reason: format!(
                    "native input {:?} has the wrong owner or type for fighter {fighter:?}",
                    source
                ),
            })
        }
        Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
            check_native_expr_fighter(left, fighter, item, errors);
            check_native_expr_fighter(right, fighter, item, errors);
        }
        _ => {}
    }
}

fn check_native_predicate_fighter(
    predicate: &Predicate,
    fighter: &str,
    item: &str,
    errors: &mut Vec<BehaviorError>,
) {
    match predicate {
        Predicate::InMotion { motion } if !super::native_inputs::motion_name_valid(motion) => {
            errors.push(BehaviorError {
                item: item.into(),
                reason: format!("{motion:?} is not a motion name"),
            });
        }
        Predicate::InMotion { .. } => {}
        Predicate::InStatus { symbol } | Predicate::TermEnabled { symbol } => {
            let kind = if matches!(predicate, Predicate::TermEnabled { .. }) {
                IdentifierKind::TransitionTerm
            } else {
                IdentifierKind::FighterStatus
            };
            if let Err(reason) = registry::resolve(fighter, symbol, kind) {
                errors.push(BehaviorError {
                    item: item.into(),
                    reason: reason.to_string(),
                });
            }
        }

        Predicate::Compare { left, right, .. } => {
            check_native_expr_fighter(left, fighter, item, errors);
            check_native_expr_fighter(right, fighter, item, errors);
        }
        Predicate::NativeFlag(source) | Predicate::DamageStatusIs(source)
            if !source.valid_for(fighter) || source.kind() != Some(NativeKind::Flag) =>
        {
            errors.push(BehaviorError {
                item: item.to_string(),
                reason: format!(
                    "native flag {:?} has the wrong owner or type for fighter {fighter:?}",
                    source
                ),
            })
        }
        Predicate::Not(inner) => check_native_predicate_fighter(inner, fighter, item, errors),
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            check_native_predicate_fighter(left, fighter, item, errors);
            check_native_predicate_fighter(right, fighter, item, errors);
        }
        Predicate::DamageIsProjectile
        | Predicate::SituationChanged
        | Predicate::MotionEnded
        | Predicate::CancelEnabled
        | Predicate::Situation { .. }
        | Predicate::Button { .. }
        | Predicate::Flag(_)
        | Predicate::EventFlag { .. }
        | Predicate::NativeFlag(_)
        | Predicate::DamageStatusIs(_)
        | Predicate::Expired(_) => {}
    }
}

fn expr_type(
    expr: &Expr,
    vars: &BTreeMap<&str, &VarDef>,
    event_scopes: &EventReadScopes<'_>,
    transition: usize,
) -> Option<VarType> {
    match expr {
        Expr::Const(value) => Some(value.kind()),
        Expr::Var(name) => vars.get(name.as_str()).map(|var| var.kind),
        Expr::TimerRemaining(name) => vars
            .get(name.as_str())
            .filter(|var| var.kind == VarType::Timer)
            .map(|_| VarType::Int),
        Expr::EventField { event, field } => event_scopes
            .resolve(transition, event, field)
            .ok()
            .filter(|kind| matches!(kind, VarType::Int | VarType::Float)),
        Expr::MotionFrame
        | Expr::StickX { .. }
        | Expr::StickY
        | Expr::RandomFloat { .. }
        | Expr::PendingDamage
        | Expr::NativeFloat(_) => Some(VarType::Float),
        Expr::StatusFrame | Expr::RandomInt { .. } | Expr::NativeInt(_) => Some(VarType::Int),
        Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
            let (left, right) = (
                expr_type(left, vars, event_scopes, transition)?,
                expr_type(right, vars, event_scopes, transition)?,
            );
            match (left, right) {
                (VarType::Float, VarType::Float) => Some(VarType::Float),
                (VarType::Int, VarType::Int) => Some(VarType::Int),
                _ => None,
            }
        }
    }
}

/// Return the statically known scalar kind of an event value in one
/// transition's receiver scope. Predicate trees are boolean-valued; their
/// internal numeric/event references are validated by the graph walk.
pub(crate) fn event_value_kind(
    graph: &BehaviorGraph,
    transition: usize,
    value: &super::event_payload::EventValue,
) -> Result<VarType, String> {
    // This helper is also called from renderer/conversion paths. Bound the
    // complete graph before allocating scope indexes or walking any value tree,
    // even when a caller bypasses the usual validation gate.
    preflight_bounds(graph)?;
    let vars: BTreeMap<&str, &VarDef> = graph
        .vars
        .iter()
        .map(|var| (var.name.as_str(), var))
        .collect();
    let mut errors = Vec::new();
    let scopes = EventReadScopes::build(graph, &mut errors);
    match value {
        super::event_payload::EventValue::Numeric(expr) => {
            check_expr_vars(expr, &vars, &scopes, transition, "event value", &mut errors);
        }
        super::event_payload::EventValue::Flag(predicate) => {
            check_predicate(
                predicate,
                &vars,
                &scopes,
                transition,
                "event value",
                &mut errors,
            );
        }
        super::event_payload::EventValue::Constant(_) => {}
    }
    if !errors.is_empty() {
        return Err(errors
            .into_iter()
            .map(|error| error.reason)
            .collect::<Vec<_>>()
            .join("; "));
    }
    match value {
        super::event_payload::EventValue::Constant(ConstValue::Float(_)) => Ok(VarType::Float),
        super::event_payload::EventValue::Constant(ConstValue::Int(_)) => Ok(VarType::Int),
        super::event_payload::EventValue::Constant(ConstValue::Flag(_))
        | super::event_payload::EventValue::Flag(_) => Ok(VarType::Flag),
        super::event_payload::EventValue::Numeric(expr) => {
            match expr_type(expr, &vars, &scopes, transition) {
                Some(kind @ (VarType::Float | VarType::Int)) => Ok(kind),
                Some(VarType::Flag | VarType::Timer) | None => {
                    Err("a Numeric event value must evaluate to an integer or float".into())
                }
            }
        }
    }
}

fn check_expr_vars(
    expr: &Expr,
    vars: &BTreeMap<&str, &VarDef>,
    event_scopes: &EventReadScopes<'_>,
    transition: usize,
    item: &str,
    errors: &mut Vec<BehaviorError>,
) {
    if matches!(expr, Expr::PendingDamage) && !event_scopes.pending_damage_allowed(transition) {
        errors.push(BehaviorError {
            item: item.into(),
            reason: "pending damage requires an exclusively damage/shield contact root transition"
                .into(),
        });
    }
    match expr {
        Expr::RandomInt { min, max } if !super::native_inputs::random_int_valid(*min, *max) => {
            errors.push(BehaviorError {
                item: item.into(),
                reason: "random integer requires bounded min <= max and span <= 1000000".into(),
            })
        }
        Expr::RandomFloat { min, max } if !super::native_inputs::random_float_valid(*min, *max) => {
            errors.push(BehaviorError {
                item: item.into(),
                reason: "random float requires finite bounded min <= max".into(),
            })
        }
        Expr::Const(ConstValue::Float(value)) if !value.is_finite() => {
            errors.push(BehaviorError {
                item: item.to_string(),
                reason: "a float constant must be finite".to_string(),
            });
        }
        Expr::Const(ConstValue::Float(value))
            if f64::from(*value).abs() > super::live_model::MAX_ABS_NUMBER =>
        {
            errors.push(BehaviorError {
                item: item.to_string(),
                reason: "a numeric constant is outside the bounded range".to_string(),
            });
        }
        Expr::Const(ConstValue::Int(value))
            if i64::from(*value).abs() > super::live_model::MAX_ABS_NUMBER as i64 =>
        {
            errors.push(BehaviorError {
                item: item.to_string(),
                reason: "an integer constant is outside the bounded range".to_string(),
            });
        }
        Expr::PendingDamage
        | Expr::StatusFrame
        | Expr::MotionFrame
        | Expr::StickX { .. }
        | Expr::StickY
        | Expr::RandomInt { .. }
        | Expr::RandomFloat { .. }
        | Expr::Const(_)
        | Expr::Var(_)
        | Expr::TimerRemaining(_)
        | Expr::EventField { .. }
        | Expr::NativeFloat(_)
        | Expr::NativeInt(_)
        | Expr::Add(_, _)
        | Expr::Sub(_, _)
        | Expr::Mul(_, _) => {}
    }
    if let Expr::Var(name) = expr {
        if !vars.contains_key(name.as_str()) {
            errors.push(BehaviorError {
                item: item.to_string(),
                reason: format!("uses variable {name:?}, which is not declared"),
            });
        }
    }
    if let Expr::TimerRemaining(name) = expr {
        match vars.get(name.as_str()).map(|var| var.kind) {
            Some(VarType::Timer) => {}
            Some(kind) => errors.push(BehaviorError {
                item: item.to_string(),
                reason: format!(
                    "timer remaining requires a Timer variable, but {name:?} is {kind:?}"
                ),
            }),
            None => errors.push(BehaviorError {
                item: item.to_string(),
                reason: format!("timer remaining references undeclared variable {name:?}"),
            }),
        }
    }
    if let Expr::EventField { event, field } = expr {
        match event_scopes.resolve(transition, event, field) {
            Ok(VarType::Int | VarType::Float) => {}
            Ok(kind) => errors.push(BehaviorError {
                item: item.to_string(),
                reason: format!("event field type {kind:?} is not a numeric expression value"),
            }),
            Err(reason) => errors.push(BehaviorError {
                item: item.to_string(),
                reason: reason.into(),
            }),
        }
    }
    match expr {
        Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
            check_expr_vars(left, vars, event_scopes, transition, item, errors);
            check_expr_vars(right, vars, event_scopes, transition, item, errors);
        }
        Expr::NativeFloat(source) if !source.is_supported() => errors.push(BehaviorError {
            item: item.to_string(),
            reason: format!("native input {:?} is unsupported", source),
        }),
        Expr::NativeInt(source)
            if !source.is_supported() || source.kind() != Some(NativeKind::Int) =>
        {
            errors.push(BehaviorError {
                item: item.to_string(),
                reason: format!("native integer input {:?} has the wrong type", source),
            })
        }
        Expr::NativeFloat(source) if source.kind() != Some(NativeKind::Float) => {
            errors.push(BehaviorError {
                item: item.to_string(),
                reason: format!("native float input {:?} has the wrong type", source),
            })
        }
        Expr::PendingDamage
        | Expr::StatusFrame
        | Expr::MotionFrame
        | Expr::StickX { .. }
        | Expr::StickY
        | Expr::RandomInt { .. }
        | Expr::RandomFloat { .. }
        | Expr::Const(_)
        | Expr::Var(_)
        | Expr::TimerRemaining(_)
        | Expr::EventField { .. }
        | Expr::NativeFloat(_)
        | Expr::NativeInt(_) => {}
    }
}

fn check_predicate(
    guard: &Predicate,
    vars: &BTreeMap<&str, &VarDef>,
    event_scopes: &EventReadScopes<'_>,
    transition: usize,
    item: &str,
    errors: &mut Vec<BehaviorError>,
) {
    match guard {
        Predicate::DamageIsProjectile => {
            if !event_scopes.pending_damage_allowed(transition) {
                errors.push(BehaviorError {
                    item: item.into(),
                    reason: "projectile damage classification requires an exclusively damage/shield contact root transition".into(),
                });
            }
        }
        Predicate::InStatus { .. }
        | Predicate::InMotion { .. }
        | Predicate::SituationChanged
        | Predicate::MotionEnded
        | Predicate::CancelEnabled
        | Predicate::TermEnabled { .. }
        | Predicate::Situation { .. }
        | Predicate::Button { .. } => {}
        Predicate::Flag(name) => match vars.get(name.as_str()) {
            None => errors.push(BehaviorError {
                item: item.to_string(),
                reason: format!("tests variable {name:?}, which is not declared"),
            }),
            Some(var) if var.kind != VarType::Flag => errors.push(BehaviorError {
                item: item.to_string(),
                reason: format!("tests {:?} as a flag, but it is {:?}", var.name, var.kind),
            }),
            Some(_) => {}
        },
        Predicate::EventFlag { event, field } => {
            match event_scopes.resolve(transition, event, field) {
                Ok(VarType::Flag) => {}
                Ok(kind) => errors.push(BehaviorError {
                    item: item.to_string(),
                    reason: format!("event field is {kind:?}, not Flag"),
                }),
                Err(reason) => errors.push(BehaviorError {
                    item: item.to_string(),
                    reason: reason.into(),
                }),
            }
        }
        Predicate::DamageStatusIs(source) => {
            if !matches!(source, NativeInput::FighterStatusIs { .. }) || !source.is_supported() {
                errors.push(BehaviorError {
                    item: item.into(),
                    reason: "contact status requires an exact registered fighter-status selector"
                        .into(),
                });
            }
            if !event_scopes.pending_damage_allowed(transition) {
                errors.push(BehaviorError {
                    item: item.into(),
                    reason: "contact status requires an exclusively damage/shield contact root transition"
                        .into(),
                });
            }
        }
        Predicate::NativeFlag(source) => {
            if !source.is_supported() || source.kind() != Some(NativeKind::Flag) {
                errors.push(BehaviorError {
                    item: item.to_string(),
                    reason: format!("native flag input {:?} has the wrong type", source),
                });
            }
        }
        Predicate::Expired(name) => match vars.get(name.as_str()) {
            None => errors.push(BehaviorError {
                item: item.to_string(),
                reason: format!("tests variable {name:?}, which is not declared"),
            }),
            Some(var) if var.kind != VarType::Timer => errors.push(BehaviorError {
                item: item.to_string(),
                reason: format!(
                    "tests {:?} for expiry, but it is {:?}, not a timer",
                    var.name, var.kind
                ),
            }),
            Some(_) => {}
        },
        Predicate::Compare { op, left, right } => {
            check_expr_vars(left, vars, event_scopes, transition, item, errors);
            check_expr_vars(right, vars, event_scopes, transition, item, errors);
            match (
                expr_type(left, vars, event_scopes, transition),
                expr_type(right, vars, event_scopes, transition),
            ) {
                (Some(VarType::Float), Some(VarType::Float))
                | (Some(VarType::Int), Some(VarType::Int)) => {}
                (left, right) => errors.push(BehaviorError {
                    item: item.to_string(),
                    reason: format!(
                        "compares {left:?} {op} {right:?}; both sides must be floats or both ints"
                    ),
                }),
            }
        }
        Predicate::Not(inner) => {
            check_predicate(inner, vars, event_scopes, transition, item, errors)
        }
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            check_predicate(left, vars, event_scopes, transition, item, errors);
            check_predicate(right, vars, event_scopes, transition, item, errors);
        }
    }
}

fn check_action(
    action: &Action,
    vars: &BTreeMap<&str, &VarDef>,
    event_scopes: &EventReadScopes<'_>,
    transition: usize,
    item: &str,
    errors: &mut Vec<BehaviorError>,
) {
    let name = match action {
        Action::Status(command) => {
            for e in command.expressions() {
                check_expr_vars(e, vars, event_scopes, transition, item, errors);
            }
            return;
        }
        Action::ThrowHeldOpponent { .. }
        | Action::ReleaseHeldOpponent
        | Action::CancelIncomingHit => return,
        Action::CombatPolicy {
            scope,
            op:
                CombatOp::SuperArmor | CombatOp::Intangible | CombatOp::Invincible | CombatOp::Clear(_),
        } => {
            if let Err(reason) = super::native_inputs::validate_policy_scope(*scope) {
                errors.push(BehaviorError {
                    item: item.into(),
                    reason,
                });
            }
            return;
        }
        Action::Movement(op) => {
            if let Err(reason) = op.validate() {
                errors.push(BehaviorError {
                    item: item.to_string(),
                    reason,
                });
            }
            return;
        }
        Action::Resource(command) => {
            check_expr_vars(
                &command.amount,
                vars,
                event_scopes,
                transition,
                item,
                errors,
            );
            let constant = match command.amount {
                Expr::Const(ConstValue::Float(v)) => command
                    .try_map(|_| Ok::<_, String>(v))
                    .and_then(|c| c.validate(c.resource.fighter())),
                _ => Ok(()),
            };
            if expr_type(&command.amount, vars, event_scopes, transition) != Some(VarType::Float) {
                errors.push(BehaviorError {
                    item: item.into(),
                    reason: format!("{} needs a Float amount", command.label()),
                });
            } else if let Err(reason) = constant {
                errors.push(BehaviorError {
                    item: item.into(),
                    reason,
                });
            }
            return;
        }
        Action::ChooseHeroMenuSpell { slot } => {
            if *slot >= super::hero_menu_commands::Command::SLOTS {
                errors.push(BehaviorError {
                    item: item.into(),
                    reason: "Hero menu slot must be 0..=3".into(),
                });
            }
            return;
        }
        Action::Item(op) => {
            if let Err(reason) = op.validate() {
                errors.push(BehaviorError {
                    item: item.to_string(),
                    reason,
                });
            }
            return;
        }
        Action::MeshVisibility { mesh, .. } => {
            if let Err(reason) = super::native_inputs::validate_mesh_name(mesh) {
                errors.push(BehaviorError {
                    item: item.to_string(),
                    reason,
                });
            }
            return;
        }
        Action::Object { op, .. } => {
            for value in op.expressions() {
                check_expr_vars(value, vars, event_scopes, transition, item, errors);
                let bounded = match value {
                    Expr::Const(ConstValue::Float(v)) => {
                        v.abs() <= super::native_inputs::ObjectOp::<Expr>::MAX_SPEED
                    }
                    _ => true,
                };
                if expr_type(value, vars, event_scopes, transition) != Some(VarType::Float)
                    || !bounded
                {
                    errors.push(BehaviorError {
                        item: item.into(),
                        reason: "object speed requires Float expressions within ±10".into(),
                    });
                }
            }
            return;
        }
        Action::AddHeldOpponentDamage { amount: rate }
        | Action::AddDamage { amount: rate }
        | Action::CombatPolicy {
            op:
                CombatOp::DamageTaken(rate)
                | CombatOp::KnockbackTaken(rate)
                | CombatOp::DamageDealt(rate)
                | CombatOp::KnockbackDealt(rate)
                | CombatOp::Stat(_, rate)
                | CombatOp::Armor(rate),
            ..
        }
        | Action::SetMotionRate { rate }
        | Action::AddSpeed { x: rate, .. } => {
            if let Action::CombatPolicy { scope, .. } = action {
                if let Err(reason) = super::native_inputs::validate_policy_scope(*scope) {
                    errors.push(BehaviorError {
                        item: item.into(),
                        reason,
                    });
                }
            }
            let expressions = match action {
                Action::AddSpeed { x, y } => vec![x, y],
                _ => vec![rate],
            };
            for value in expressions {
                check_expr_vars(value, vars, event_scopes, transition, item, errors);
                if expr_type(value, vars, event_scopes, transition) != Some(VarType::Float) {
                    errors.push(BehaviorError {
                        item: item.into(),
                        reason: "native scalar actions require Float expressions".into(),
                    });
                }
                if let Expr::Const(ConstValue::Float(v)) = value {
                    let valid = match action {
                        Action::AddHeldOpponentDamage { .. } => {
                            v.is_finite() && (0.0..=999.0).contains(v)
                        }
                        Action::AddDamage { .. } => v.is_finite() && v.abs() <= 999.0,
                        Action::CombatPolicy { op, .. } => {
                            op.bounds().is_some_and(|range| range.contains(v))
                        }
                        Action::SetMotionRate { .. } => (0.05..=20.0).contains(v),
                        _ => v.is_finite() && v.abs() <= 10.0,
                    };
                    if !valid {
                        errors.push(BehaviorError {
                            item: item.into(),
                            reason: "native scalar action is outside its bounds".into(),
                        });
                    }
                }
            }
            return;
        }
        Action::SetNative { input, value } | Action::AddNative { input, value } => {
            check_expr_vars(value, vars, event_scopes, transition, item, errors);
            let kind = match input.kind() {
                Some(NativeKind::Float) => Some(VarType::Float),
                Some(NativeKind::Int) => Some(VarType::Int),
                _ => None,
            };
            if kind.is_none() || expr_type(value, vars, event_scopes, transition) != kind {
                errors.push(BehaviorError {
                    item: item.into(),
                    reason:
                        "native numeric write requires matching Float/Int expression and Work field"
                            .into(),
                });
            }
            return;
        }
        Action::SetNativeFlag { input, value } => {
            check_predicate(value, vars, event_scopes, transition, item, errors);
            if input.kind() != Some(NativeKind::Flag) {
                errors.push(BehaviorError {
                    item: item.into(),
                    reason: "native flag write requires a Flag Work field".into(),
                });
            }
            return;
        }
        Action::Set { var, .. }
        | Action::SetFlag { var, .. }
        | Action::Add { var, .. }
        | Action::Raise { var }
        | Action::Clear { var }
        | Action::Reset { var } => var,
        Action::GenerateArticle { .. }
        | Action::RemoveArticle { .. }
        | Action::RemoveNewestArticle { .. }
        | Action::ArticleMovement { .. }
        | Action::RequestArticleStatus { .. } => return,
        Action::EmitEvent { name } => {
            if name.trim().is_empty() || name.len() > 48 || name.contains('\0') {
                errors.push(BehaviorError {
                    item: item.to_string(),
                    reason: "named event must be non-empty, NUL-free, and at most 48 bytes"
                        .to_string(),
                });
            }
            return;
        }
        Action::EmitEventArguments { name, arguments } => {
            if name.trim().is_empty() || name.len() > 48 || name.contains('\0') {
                errors.push(BehaviorError {
                    item: item.to_string(),
                    reason: "named event must be non-empty, NUL-free, and at most 48 bytes"
                        .to_string(),
                });
            }
            if let Err(argument_errors) =
                super::event_payload::validate_argument_shape(item, name, arguments)
            {
                errors.extend(argument_errors.into_iter().map(|error| BehaviorError {
                    item: item.to_string(),
                    reason: error.to_string(),
                }));
            }
            return;
        }
        Action::EmitEventValues { name, arguments } => {
            if name.trim().is_empty() || name.len() > 48 || name.contains('\0') {
                errors.push(BehaviorError {
                    item: item.to_string(),
                    reason: "named event must be non-empty, NUL-free, and at most 48 bytes"
                        .to_string(),
                });
            }
            if let Err(argument_errors) =
                super::event_payload::validate_value_argument_shape(item, name, arguments)
            {
                errors.extend(argument_errors.into_iter().map(|error| BehaviorError {
                    item: item.to_string(),
                    reason: error.to_string(),
                }));
            }
            for argument in arguments {
                match &argument.value {
                    super::event_payload::EventValue::Constant(_) => {}
                    super::event_payload::EventValue::Numeric(expr) => {
                        check_expr_vars(expr, vars, event_scopes, transition, item, errors);
                        if !matches!(
                            expr_type(expr, vars, event_scopes, transition),
                            Some(VarType::Float | VarType::Int)
                        ) {
                            errors.push(BehaviorError {
                                item: item.to_string(),
                                reason:
                                    "a Numeric event value must evaluate to an integer or float"
                                        .to_string(),
                            });
                        }
                    }
                    super::event_payload::EventValue::Flag(predicate) => {
                        check_predicate(predicate, vars, event_scopes, transition, item, errors);
                    }
                }
            }
            return;
        }
        Action::RequestFighterStatus { .. } => return,
        Action::RequestWeaponStatus { target, .. } => {
            if target.battle_object_id == 0 {
                errors.push(BehaviorError {
                    item: item.to_string(),
                    reason: "weapon-status action needs a nonzero explicit weapon battle-object id"
                        .to_string(),
                });
            }
            return;
        }
    };
    let Some(var) = vars.get(name.as_str()) else {
        errors.push(BehaviorError {
            item: item.to_string(),
            reason: format!("writes variable {name:?}, which is not declared"),
        });
        return;
    };
    match action {
        Action::Set { value, .. } => {
            // Unknown reads are reported here; the type error below only
            // fires when every variable is known, so one bad name does not
            // cascade into a bogus mismatch as well.
            check_expr_vars(value, vars, event_scopes, transition, item, errors);
            let mut missing = false;
            collect_vars(value, &mut missing, vars, event_scopes, transition);
            let writes = matches!(
                (var.kind, expr_type(value, vars, event_scopes, transition)),
                (VarType::Float, Some(VarType::Float))
                    | (VarType::Int, Some(VarType::Int))
                    | (VarType::Timer, Some(VarType::Int))
            );
            if !missing && !writes {
                errors.push(BehaviorError {
                    item: item.to_string(),
                    reason: format!(
                        "writes {:?} ({:?}) with an incompatible value; timers start from ints",
                        var.name, var.kind
                    ),
                });
            }
        }
        Action::SetFlag { value, .. } => {
            check_predicate(value, vars, event_scopes, transition, item, errors);
            if var.kind != VarType::Flag {
                errors.push(BehaviorError {
                    item: item.to_string(),
                    reason: format!(
                        "assigns a predicate to {:?} ({:?}), which is not a flag",
                        var.name, var.kind
                    ),
                });
            }
        }
        Action::Add { value, .. } => {
            check_expr_vars(value, vars, event_scopes, transition, item, errors);
            let mut missing = false;
            collect_vars(value, &mut missing, vars, event_scopes, transition);
            let steps = matches!(
                (var.kind, expr_type(value, vars, event_scopes, transition)),
                (VarType::Float, Some(VarType::Float)) | (VarType::Int, Some(VarType::Int))
            );
            if !missing && !steps {
                errors.push(BehaviorError {
                    item: item.to_string(),
                    reason: format!(
                        "steps {:?} ({:?}) with an incompatible value; only floats and ints step",
                        var.name, var.kind
                    ),
                });
            }
        }
        Action::Raise { .. } | Action::Clear { .. } => {
            if var.kind != VarType::Flag {
                errors.push(BehaviorError {
                    item: item.to_string(),
                    reason: format!("flips {:?}, but it is {:?}, not a flag", var.name, var.kind),
                });
            }
        }
        Action::Reset { .. } => {}
        Action::Status(_)
        | Action::AddHeldOpponentDamage { .. }
        | Action::AddDamage { .. }
        | Action::CombatPolicy { .. }
        | Action::ThrowHeldOpponent { .. }
        | Action::ReleaseHeldOpponent
        | Action::CancelIncomingHit
        | Action::Movement(_)
        | Action::SetMotionRate { .. }
        | Action::AddSpeed { .. }
        | Action::SetNative { .. }
        | Action::AddNative { .. }
        | Action::SetNativeFlag { .. }
        | Action::GenerateArticle { .. }
        | Action::RemoveArticle { .. }
        | Action::RemoveNewestArticle { .. }
        | Action::ArticleMovement { .. }
        | Action::Object { .. }
        | Action::RequestArticleStatus { .. }
        | Action::RequestFighterStatus { .. }
        | Action::RequestWeaponStatus { .. }
        | Action::EmitEvent { .. }
        | Action::EmitEventArguments { .. }
        | Action::EmitEventValues { .. }
        | Action::Resource(_)
        | Action::Item(_)
        | Action::MeshVisibility { .. }
        | Action::ChooseHeroMenuSpell { .. } => {
            unreachable!("handled above")
        }
    }
}

/// Whether every variable an expression reads is declared. Unknown reads are
/// reported by [`check_expr_vars`]; this only decides whether a type error
/// would cascade noise on top.
fn collect_vars(
    expr: &Expr,
    missing: &mut bool,
    vars: &BTreeMap<&str, &VarDef>,
    event_scopes: &EventReadScopes<'_>,
    transition: usize,
) {
    match expr {
        Expr::StatusFrame
        | Expr::MotionFrame
        | Expr::StickX { .. }
        | Expr::StickY
        | Expr::RandomInt { .. }
        | Expr::RandomFloat { .. } => {}
        Expr::PendingDamage => {}
        Expr::Const(_) | Expr::NativeFloat(_) | Expr::NativeInt(_) => {}
        Expr::Var(name) => {
            if !vars.contains_key(name.as_str()) {
                *missing = true;
            }
        }
        Expr::TimerRemaining(name) => {
            if !vars
                .get(name.as_str())
                .is_some_and(|var| var.kind == VarType::Timer)
            {
                *missing = true;
            }
        }
        Expr::EventField { event, field } => {
            if !matches!(
                event_scopes.resolve(transition, event, field),
                Ok(VarType::Int | VarType::Float)
            ) {
                *missing = true;
            }
        }
        Expr::Add(left, right) | Expr::Sub(left, right) | Expr::Mul(left, right) => {
            collect_vars(left, missing, vars, event_scopes, transition);
            collect_vars(right, missing, vars, event_scopes, transition);
        }
    }
}

/// Reject instant cycles: loops of transitions that all fire immediately and
/// would spin forever without advancing a frame. `on_frame` edges break the
/// loop, so only non-`on_frame` edges participate.
fn check_instant_cycles(
    graph: &BehaviorGraph,
    states: &BTreeSet<&str>,
    errors: &mut Vec<BehaviorError>,
) {
    let mut edges: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let event_transitions: BTreeSet<usize> = graph
        .emission_plan
        .event_bindings
        .iter()
        .flat_map(|binding| binding.transitions.iter().copied())
        .collect();
    for (index, transition) in graph.transitions.iter().enumerate() {
        // Emitting queues a named event for the bounded dispatch phase after
        // the current source phase. It therefore breaks the immediate edge
        // just like a receiver, even when the receiver itself transitions to
        // the same state.
        let emits_event = transition.actions.iter().any(|action| {
            matches!(
                action,
                Action::EmitEvent { .. }
                    | Action::EmitEventArguments { .. }
                    | Action::EmitEventValues { .. }
            )
        });
        if transition.on_frame
            || event_transitions.contains(&index)
            || emits_event
            || super::status_model::phase(
                graph.states.iter().filter_map(|s| s.status.as_ref()),
                index,
            )
            .is_some()
        {
            continue;
        }
        if states.contains(transition.from.as_str()) && states.contains(transition.to.as_str()) {
            edges
                .entry(transition.from.as_str())
                .or_default()
                .push(transition.to.as_str());
        }
    }
    // Iterative depth-first search with an explicit stack; gray marks the
    // current path, so reaching a gray node names the loop.
    #[derive(PartialEq)]
    enum Mark {
        Gray,
        Black,
    }
    let mut marks: BTreeMap<&str, Mark> = BTreeMap::new();
    for root in states.iter() {
        if marks.contains_key(root) {
            continue;
        }
        let mut stack: Vec<(&str, bool)> = vec![(root, false)];
        let mut path: Vec<&str> = Vec::new();
        while let Some((node, exiting)) = stack.pop() {
            if exiting {
                marks.insert(node, Mark::Black);
                path.pop();
                continue;
            }
            if marks.get(node) == Some(&Mark::Black) {
                continue;
            }
            if marks.get(node) == Some(&Mark::Gray) {
                let start = path.iter().position(|step| *step == node).unwrap_or(0);
                let mut cycle: Vec<&str> = path[start..].to_vec();
                cycle.push(node);
                errors.push(BehaviorError {
                    item: "graph".to_string(),
                    reason: format!(
                        "instant cycle {} with no frame boundary; mark one transition on_frame",
                        cycle.join(" -> ")
                    ),
                });
                continue;
            }
            marks.insert(node, Mark::Gray);
            path.push(node);
            stack.push((node, true));
            for next in edges.get(node).cloned().unwrap_or_default() {
                stack.push((next, false));
            }
        }
    }
}

pub(crate) fn requires_controller_runtime(graph: &BehaviorGraph) -> bool {
    if graph.emission_plan.bindings.iter().any(|b| {
        matches!(
            b.hook,
            super::hooks::HookName::OnIncomingHit
                | super::hooks::HookName::OnReflect
                | super::hooks::HookName::OnAbsorb
        )
    }) {
        return true;
    }
    if preflight_bounds(graph).is_err() {
        return false;
    }
    let features = required_native_features(graph);
    if features.contains(super::status_model::FEATURE) {
        return true;
    }
    if [
        super::resource_commands::FEATURE,
        super::native_inputs::ITEM_EVENTS_FEATURE,
        super::native_inputs::COMMAND_INPUT_FEATURE,
        super::native_inputs::HELD_ITEM_KIND_FEATURE,
        super::native_inputs::MOVEMENT_FEATURE,
        super::native_inputs::GRAB_EVENTS_FEATURE,
        super::native_inputs::FIGHTER_STOCKS_FEATURE,
        super::native_inputs::OWNED_ARTICLES_FEATURE,
        super::native_inputs::HELD_OPPONENT_FEATURE,
        super::native_inputs::PARTNER_FEATURE,
        super::native_inputs::OPPONENT_FEATURE,
        super::native_inputs::ITEMS_FEATURE,
    ]
    .iter()
    .any(|f| features.contains(f))
    {
        return true;
    }
    fn expr(e: &Expr) -> bool {
        match e {
            Expr::StatusFrame
            | Expr::MotionFrame
            | Expr::StickX { .. }
            | Expr::StickY
            | Expr::RandomInt { .. }
            | Expr::RandomFloat { .. } => true,
            Expr::Add(a, b) | Expr::Sub(a, b) | Expr::Mul(a, b) => expr(a) || expr(b),
            _ => false,
        }
    }
    fn pred(p: &Predicate) -> bool {
        match p {
            Predicate::DamageIsProjectile
            | Predicate::InStatus { .. }
            | Predicate::InMotion { .. }
            | Predicate::SituationChanged
            | Predicate::MotionEnded
            | Predicate::CancelEnabled
            | Predicate::TermEnabled { .. }
            | Predicate::Situation { .. }
            | Predicate::Button { .. } => true,
            Predicate::Compare { left, right, .. } => expr(left) || expr(right),
            Predicate::Not(p) => pred(p),
            Predicate::And(a, b) | Predicate::Or(a, b) => pred(a) || pred(b),
            _ => false,
        }
    }
    graph.transitions.iter().any(|t| {
        pred(&t.guard)
            || t.actions.iter().any(|a| match a {
                Action::Status(_)
                | Action::AddHeldOpponentDamage { .. }
                | Action::AddDamage { .. }
                | Action::CombatPolicy { .. }
                | Action::ThrowHeldOpponent { .. }
                | Action::ReleaseHeldOpponent
                | Action::CancelIncomingHit
                | Action::Movement(_)
                | Action::SetMotionRate { .. }
                | Action::AddSpeed { .. }
                | Action::Object { .. }
                | Action::Resource(_)
                | Action::ChooseHeroMenuSpell { .. }
                | Action::SetNative { .. }
                | Action::AddNative { .. }
                | Action::SetNativeFlag { .. } => true,
                Action::Set { value, .. } | Action::Add { value, .. } => expr(value),
                Action::SetFlag { value, .. } => pred(value),
                Action::EmitEventValues { arguments, .. } => {
                    arguments.iter().any(|a| match &a.value {
                        super::event_payload::EventValue::Numeric(e) => expr(e),
                        super::event_payload::EventValue::Flag(p) => pred(p),
                        _ => false,
                    })
                }
                _ => false,
            })
    })
}

fn transition_uses_input(t: &Transition) -> bool {
    transition_uses_reads(t, false, true, false, false)
}
fn transition_uses_phase(t: &Transition) -> bool {
    transition_uses_reads(t, true, true, false, false)
}
fn transition_uses_reads(
    t: &Transition,
    phase_only: bool,
    phase_reads: bool,
    cancel_only: bool,
    command_only: bool,
) -> bool {
    enum Node<'a> {
        Expr(&'a Expr, usize),
        Predicate(&'a Predicate, usize),
    }
    let mut pending = vec![Node::Predicate(&t.guard, 1)];
    for action in &t.actions {
        if phase_only
            && matches!(
                action,
                Action::Status(_)
                    | Action::AddHeldOpponentDamage { .. }
                    | Action::AddDamage { .. }
                    | Action::CombatPolicy { .. }
                    | Action::ThrowHeldOpponent { .. }
                    | Action::ReleaseHeldOpponent
                    | Action::Movement(_)
                    | Action::SetMotionRate { .. }
                    | Action::AddSpeed { .. }
            )
        {
            return true;
        }
        pending.extend(action.operands().into_iter().map(|operand| match operand {
            Operand::Expr(e) => Node::Expr(e, 1),
            Operand::Predicate(p) => Node::Predicate(p, 1),
        }));
    }
    let mut remaining = super::live_model::MAX_EXPR_NODES;
    while let Some(node) = pending.pop() {
        if remaining == 0 {
            break;
        }
        remaining -= 1;
        match node {
            Node::Expr(_, d) | Node::Predicate(_, d) if d > super::live_model::MAX_EXPR_DEPTH => {
                continue
            }
            Node::Predicate(
                Predicate::NativeFlag(super::native_inputs::NativeInput::Command { .. }),
                _,
            ) if command_only => return true,
            Node::Predicate(Predicate::CancelEnabled | Predicate::TermEnabled { .. }, _)
                if cancel_only =>
            {
                return true
            }
            Node::Expr(Expr::StatusFrame | Expr::MotionFrame, _)
            | Node::Predicate(
                Predicate::InStatus { .. }
                | Predicate::InMotion { .. }
                | Predicate::SituationChanged
                | Predicate::MotionEnded
                | Predicate::CancelEnabled
                | Predicate::TermEnabled { .. }
                | Predicate::Situation { .. },
                _,
            ) if phase_reads => return true,
            Node::Expr(Expr::StickX { .. } | Expr::StickY, _)
            | Node::Predicate(
                Predicate::Button { .. }
                | Predicate::NativeFlag(super::native_inputs::NativeInput::Command { .. }),
                _,
            ) if !phase_only && !cancel_only && !command_only => return true,
            Node::Expr(Expr::Add(a, b) | Expr::Sub(a, b) | Expr::Mul(a, b), d) => {
                pending.push(Node::Expr(a, d + 1));
                pending.push(Node::Expr(b, d + 1));
            }
            Node::Predicate(Predicate::Compare { left, right, .. }, d) => {
                pending.push(Node::Expr(left, d + 1));
                pending.push(Node::Expr(right, d + 1));
            }
            Node::Predicate(Predicate::Not(p), d) => pending.push(Node::Predicate(p, d + 1)),
            Node::Predicate(Predicate::And(a, b) | Predicate::Or(a, b), d) => {
                pending.push(Node::Predicate(a, d + 1));
                pending.push(Node::Predicate(b, d + 1));
            }
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gimmicks::{
        codegen::EventBinding,
        event_payload::{EventValue, FieldDef, ValueArgument},
    };

    fn graph() -> BehaviorGraph {
        let mut graph = crate::gimmicks::templates::Preset::Charge.graph();
        graph.vars.extend([
            VarDef {
                keep_across_form_switch: false,
                name: "duration".into(),
                kind: VarType::Timer,
                initial: ConstValue::Int(900),
            },
            VarDef {
                keep_across_form_switch: false,
                name: "count".into(),
                kind: VarType::Int,
                initial: ConstValue::Int(0),
            },
        ]);
        graph
    }

    fn rejects(graph: &BehaviorGraph, expected: &str) {
        let errors = validate(graph);
        assert!(
            errors.iter().any(|e| e.to_string().contains(expected)),
            "{expected}: {errors:?}"
        );
    }

    fn compare(left: Expr, right: Expr) -> Predicate {
        Predicate::Compare {
            op: CmpOp::Gte,
            left,
            right,
        }
    }

    #[test]
    fn structural_and_type_failures_identify_the_invalid_item() {
        type Mutation = fn(&mut BehaviorGraph);
        for (change, expected) in [
            ((|g| g.initial = "missing".into()) as Mutation, "missing"),
            (|g| g.states.push(g.states[0].clone()), "more than once"),
            (|g| g.vars.push(g.vars[0].clone()), "gauge"),
            (|g| g.transitions[0].to = "nowhere".into(), "nowhere"),
            (
                |g| g.vars[0].initial = ConstValue::Float(f32::NAN),
                "initializer must be finite",
            ),
            (
                |g| {
                    g.transitions[0].guard = compare(
                        Expr::Var("gauge".into()),
                        Expr::Const(ConstValue::Float(f32::INFINITY)),
                    )
                },
                "constant must be finite",
            ),
            (
                |g| {
                    g.transitions[0].actions.push(Action::Set {
                        var: "gauge".into(),
                        value: Expr::Const(ConstValue::Int(1)),
                    })
                },
                "gauge",
            ),
            (
                |g| {
                    g.transitions[0].actions.push(Action::Add {
                        var: "ready".into(),
                        value: Expr::Const(ConstValue::Float(1.0)),
                    })
                },
                "only floats and ints step",
            ),
            (
                |g| {
                    g.transitions[0].actions.push(Action::Raise {
                        var: "gauge".into(),
                    })
                },
                "not a flag",
            ),
            (
                |g| {
                    g.transitions[0].guard =
                        compare(Expr::Var("gauge".into()), Expr::Const(ConstValue::Int(10)))
                },
                "both sides",
            ),
            (
                |g| g.transitions[0].guard = Predicate::Expired("gauge".into()),
                "not a timer",
            ),
            (
                |g| g.transitions[0].guard = Predicate::Flag("gauge".into()),
                "as a flag",
            ),
            (
                |g| {
                    g.transitions[0].actions.push(Action::Set {
                        var: "duration".into(),
                        value: Expr::Const(ConstValue::Float(1.0)),
                    })
                },
                "timers start from ints",
            ),
        ] {
            let mut invalid = graph();
            change(&mut invalid);
            rejects(&invalid, expected);
        }
        assert!(validate(&graph()).is_empty());
        assert!(validate(&BehaviorGraph::default()).is_empty());
    }

    #[test]
    fn timer_reads_require_declared_timers_and_bounded_names() {
        for (name, expected) in [
            ("duration", None),
            ("gauge", Some("requires a Timer")),
            ("missing", Some("undeclared")),
        ] {
            let mut graph = graph();
            graph.transitions[0].guard = compare(
                Expr::TimerRemaining(name.into()),
                Expr::Const(ConstValue::Int(0)),
            );
            if let Some(expected) = expected {
                rejects(&graph, expected);
            } else {
                assert!(validate(&graph).is_empty());
            }
        }
        for name in [
            "duration\0bad".to_owned(),
            "x".repeat(crate::gimmicks::live_model::MAX_NAME_BYTES + 1),
        ] {
            let mut graph = graph();
            graph.transitions[0].guard =
                compare(Expr::TimerRemaining(name), Expr::Const(ConstValue::Int(0)));
            assert!(preflight_bounds(&graph).is_err());
        }
    }

    #[test]
    fn event_fields_are_typed_and_scoped_to_the_unique_receiver() {
        let field = |event: &str, field: &str| Expr::EventField {
            event: event.into(),
            field: field.into(),
        };
        let flag = |event: &str, field: &str| Predicate::EventFlag {
            event: event.into(),
            field: field.into(),
        };
        let mut graph = graph();
        graph.emission_plan = Default::default();
        graph.emission_plan.event_bindings.push(EventBinding {
            name: "ready".into(),
            transitions: vec![0],
            fields: vec![
                FieldDef {
                    name: "count".into(),
                    kind: VarType::Int,
                },
                FieldDef {
                    name: "active".into(),
                    kind: VarType::Flag,
                },
            ],
        });
        graph.transitions[0].guard = Predicate::And(
            Box::new(flag("ready", "active")),
            Box::new(compare(
                field("ready", "count"),
                Expr::Const(ConstValue::Int(0)),
            )),
        );
        graph.transitions[0].actions = vec![Action::EmitEventValues {
            name: "ready".into(),
            arguments: vec![
                ValueArgument {
                    field: "count".into(),
                    value: EventValue::Numeric(field("ready", "count")),
                },
                ValueArgument {
                    field: "active".into(),
                    value: EventValue::Flag(flag("ready", "active")),
                },
            ],
        }];
        assert!(validate(&graph).is_empty());
        for (guard, expected) in [
            (flag("missing", "hidden"), "unknown receiver event"),
            (
                compare(field("ready", "missing"), Expr::Const(ConstValue::Int(0))),
                "unknown receiver field",
            ),
            (flag("ready", "count"), "not Flag"),
            (
                compare(field("ready", "active"), Expr::Const(ConstValue::Int(0))),
                "not a numeric expression value",
            ),
        ] {
            let mut invalid = graph.clone();
            invalid.transitions[0].guard = guard;
            rejects(&invalid, expected);
        }
        for copy_action in [false, true] {
            let mut invalid = graph.clone();
            if copy_action {
                invalid.transitions[1].actions = invalid.transitions[0].actions.clone();
            } else {
                invalid.transitions[1].guard = invalid.transitions[0].guard.clone();
            }
            rejects(&invalid, "outside its exact receiver transition scope");
        }
        let restored: BehaviorGraph =
            serde_json::from_value(serde_json::to_value(&graph).unwrap()).unwrap();
        assert_eq!(restored, graph);
        assert_eq!(
            serde_json::to_string(&Action::EmitEvent {
                name: "ready".into()
            })
            .unwrap(),
            r#"{"EmitEvent":{"name":"ready"}}"#
        );
    }

    #[test]
    fn instant_cycles_fail_and_frame_edges_break_cycles() {
        for self_loop in [false, true] {
            let mut graph = graph();
            graph.transitions.clear();
            graph.states.push(StateDef {
                status: None,
                name: "other".into(),
            });
            let names = graph
                .states
                .iter()
                .map(|s| s.name.clone())
                .collect::<Vec<_>>();
            for (from, to) in if self_loop {
                vec![(&names[0], &names[0])]
            } else {
                vec![(&names[0], &names[1]), (&names[1], &names[0])]
            } {
                graph.transitions.push(Transition {
                    from: from.clone(),
                    to: to.clone(),
                    guard: Predicate::Flag("ready".into()),
                    on_frame: false,
                    actions: vec![],
                });
            }
            rejects(&graph, "cycle");
            graph.transitions[0].on_frame = true;
            assert!(validate(&graph).is_empty());
        }
    }

    #[test]
    fn native_actions_require_exact_owner_category_and_explicit_weapon_target() {
        for (symbol, target, expected) in [
            ("WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR", 0x2000_0001, None),
            (
                "WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR",
                0,
                Some("nonzero explicit weapon battle-object id"),
            ),
            ("FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE", 1, Some("WrongKind")),
            (
                "WEAPON_MARIO_FIREBALL_STATUS_KIND_REGULAR",
                1,
                Some("WrongFighter"),
            ),
        ] {
            let mut graph = graph();
            graph.transitions[0].actions = vec![Action::RequestWeaponStatus {
                target: super::super::live_model::WeaponTarget {
                    battle_object_id: target,
                },
                symbol: symbol.into(),
            }];
            let errors = validate_for_fighter("cloud", &graph);
            if let Some(expected) = expected {
                assert!(
                    errors.iter().any(|e| e.to_string().contains(expected)),
                    "{errors:?}"
                );
            } else {
                assert!(errors.is_empty(), "{errors:?}");
            }
        }
        for (symbol, expected) in [
            ("FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE", None),
            ("WEAPON_CLOUD_WAVE_STATUS_KIND_REGULAR", Some("WrongKind")),
            (
                "FIGHTER_BRAVE_GENERATE_ARTICLE_FIREBALL",
                Some("WrongFighter"),
            ),
        ] {
            let mut graph = graph();
            graph.transitions[0].actions = vec![
                Action::GenerateArticle {
                    symbol: symbol.into(),
                },
                Action::RemoveArticle {
                    symbol: symbol.into(),
                },
            ];
            let errors = validate_for_fighter("cloud", &graph);
            if let Some(expected) = expected {
                assert!(
                    errors.iter().any(|e| e.to_string().contains(expected)),
                    "{errors:?}"
                );
            } else {
                assert!(errors.is_empty());
            }
        }
    }
}

#[cfg(test)]
mod status_compat_tests {
    #[test]
    fn legacy_state_round_trip_has_no_status_field() {
        let state: super::StateDef = serde_json::from_str(r#"{"name":"idle"}"#).unwrap();
        assert!(state.status.is_none());
        assert_eq!(serde_json::to_string(&state).unwrap(), r#"{"name":"idle"}"#);
        let graph = crate::gimmicks::fixtures::charge_graph();
        let encoded = serde_json::to_string(&graph).unwrap();
        assert_eq!(
            serde_json::from_str::<super::BehaviorGraph>(&encoded).unwrap(),
            graph
        );
    }
}
