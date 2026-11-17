//! Bounded, pure coordination for independent persistent controller charts.
//!
//! Each chart remains an ordinary [`LiveProgram`] with its own [`LiveState`].
//! This module coordinates chart selection and atomic actor-state commits; it
//! never performs engine actions or owns actor identity/lifecycle storage.
//! [`LiveProgram::validate`] enforces only model structure, bounds, and types;
//! it does not grant native-symbol permission, validate actor identity, or
//! validate a transport envelope. Callers must retain those adapter guards.

#![allow(dead_code)] // This foundation is wired by follow-up desktop/plugin slices.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::live_model::{
    Action, EvaluationError, HookName, LiveProgram, LiveState, ResourceSource, StepResult,
    MAX_ACTIONS_PER_TRANSITION, MAX_NATIVE_INPUTS, MAX_VARIABLES,
};
use super::native_inputs::{ButtonEdge, InputButton, NativeInput, NativeSnapshot};

pub const MAX_CHARTS: usize = 8;
pub const MAX_AGGREGATE_VARIABLES: usize = 128;
pub const MAX_AGGREGATE_TRANSITIONS: usize = 256;
pub const MAX_AGGREGATE_EVENT_BINDINGS: usize = 64;
pub const MAX_UNION_NATIVE_INPUTS: usize = 64;
/// Caps the total root-step engine operations returned to an adapter.
pub const MAX_ENGINE_ACTIONS_PER_STEP: usize = 256;
/// Every chart is individually validated before this checked aggregate is
/// considered. This makes the maximum authored action table explicit.
pub const MAX_AGGREGATE_AUTHORED_ACTIONS: usize =
    MAX_AGGREGATE_TRANSITIONS * MAX_ACTIONS_PER_TRANSITION;

/// Validated programs, ordered by stable chart ID. `main` is an ordinary
/// caller-selectable chart ID reserved for legacy adapters; this coordinator
/// does not require it to be present.
#[derive(Clone, Debug)]
pub struct ControllerCoordinator {
    programs: BTreeMap<String, Arc<LiveProgram>>,
    state_token: Arc<()>,
}

/// Opaque per-actor chart states bound to the coordinator that created them.
/// The maps remain private so callers cannot forge partial or mismatched state;
/// clone is intentionally supported for transactional callers/tests.
#[derive(Clone, Debug)]
pub struct ControllerState {
    token: Arc<()>,
    charts: BTreeMap<String, LiveState>,
    sampled_edges: BTreeSet<(InputButton, ButtonEdge)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartStepResult {
    pub chart_id: String,
    pub result: StepResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartEngineAction {
    pub chart_id: String,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ControllerStepResult {
    /// Stable chart-ID order for hooks/frames; exactly one entry for a named
    /// event. Each chart's internal emitted-event queue is kept local by the
    /// underlying evaluator.
    pub charts: Vec<ChartStepResult>,
    /// Chart-tagged engine operations in deterministic chart and transition
    /// order. The adapter must still perform its own actor-identity checks.
    pub engine_actions: Vec<ChartEngineAction>,
}

impl ControllerCoordinator {
    /// False when this costume slot is outside a chart that names its costumes.
    #[allow(dead_code)] // Called by the plugin only.
    pub fn runs_for_costume(&self, costume: u8) -> bool {
        self.programs
            .values()
            .all(|program| program.costumes.is_empty() || program.costumes.contains(&costume))
    }

    /// Validate all programs and aggregate budgets before taking ownership of
    /// any program in the coordinator.
    pub fn new(programs: BTreeMap<String, LiveProgram>) -> Result<Self, String> {
        validate_program_set(&programs)?;
        let programs = programs
            .into_iter()
            .map(|(chart_id, program)| (chart_id, Arc::new(program)))
            .collect();
        Ok(Self {
            programs,
            state_token: Arc::new(()),
        })
    }

    pub fn chart_ids(&self) -> impl Iterator<Item = &str> {
        self.programs.keys().map(String::as_str)
    }

    pub fn program(&self, chart_id: &str) -> Option<&LiveProgram> {
        self.programs.get(chart_id).map(Arc::as_ref)
    }

    /// Distinct reviewed native sources needed by these charts, in stable
    /// order, ready for one immutable dispatch snapshot sampler.
    pub fn native_inputs(&self) -> Vec<NativeInput> {
        self.programs
            .values()
            .flat_map(|program| program.native_inputs.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn initial_state(&self) -> ControllerState {
        ControllerState {
            token: Arc::clone(&self.state_token),
            sampled_edges: BTreeSet::new(),
            charts: self
                .programs
                .iter()
                .map(|(chart_id, program)| (chart_id.clone(), program.initial_state()))
                .collect(),
        }
    }

    /// Copy only explicitly shared variables between matching controller IDs.
    pub fn carry_form_variables(
        &self,
        state: &mut ControllerState,
        outgoing: &Self,
        old: &ControllerState,
    ) -> Result<(), String> {
        self.check_state(state)?;
        outgoing.check_state(old)?;
        for (chart, program) in &self.programs {
            let Some(previous) = outgoing.program(chart) else {
                continue;
            };
            for variable in program.vars.iter().filter(|v| v.keep_across_form_switch) {
                if previous.vars.iter().any(|v| {
                    v.keep_across_form_switch && v.name == variable.name && v.kind == variable.kind
                }) {
                    if let Some(value) = old.charts[chart].values.get(&variable.name) {
                        state
                            .charts
                            .get_mut(chart)
                            .unwrap()
                            .values
                            .insert(variable.name.clone(), *value);
                    }
                }
            }
        }
        Ok(())
    }

    /// Evaluate one frame in every chart, independently decrementing each
    /// chart's timers and preserving its own causal timeout-expiry set.
    pub fn step_frame(
        &self,
        state: &mut ControllerState,
        snapshot: &NativeSnapshot,
    ) -> Result<ControllerStepResult, String> {
        self.check_state(state)?;
        let mut snapshot = snapshot.clone();
        let edges = snapshot.controller.as_mut().map(|input| {
            let edges = input
                .buttons
                .iter()
                .filter(|((_, edge), value)| *edge != ButtonEdge::Held && **value)
                .map(|(key, _)| *key)
                .collect();
            // Trigger/release bits may survive several status callbacks (or
            // stopped frames). All charts share one consumption of each edge.
            for edge in &state.sampled_edges {
                if let Some(value) = input.buttons.get_mut(edge) {
                    *value = false;
                }
            }
            edges
        });
        let selected = self
            .programs
            .iter()
            .filter(|(chart, p)| {
                !p.status_bindings.iter().any(|b| {
                    b.binding.policy == super::status_model::Policy::Owned
                        && state.charts.get(*chart).is_some_and(|s| s.state == b.state)
                })
            })
            .map(|(chart, _)| chart.clone())
            .collect::<Vec<_>>();
        let result = self.step_selected(state, &selected, false, |_, program, chart_state| {
            program.step_frame_with_snapshot(chart_state, &snapshot)
        })?;
        if let Some(edges) = edges {
            state.sampled_edges = edges;
        }
        Ok(result)
    }

    pub fn status_owner(&self, status: i32) -> Option<(&str, &super::status_model::StateStatus)> {
        self.programs.iter().find_map(|(chart, program)| {
            program
                .status_bindings
                .iter()
                .find(|b| {
                    b.binding
                        .target
                        .resolve(&program.fighter)
                        .is_ok_and(|id| id == status as u32)
                })
                .map(|b| (chart.as_str(), b))
        })
    }

    pub fn step_status_phase(
        &self,
        state: &mut ControllerState,
        status: i32,
        phase: super::status_model::Phase,
        snapshot: &NativeSnapshot,
    ) -> Result<ControllerStepResult, String> {
        let (chart, entry) = self
            .status_owner(status)
            .ok_or("status has no controller owner")?;
        self.step_selected(state, &[chart.to_owned()], false, |_, program, state| {
            program.step_status_phase(state, &entry.state, phase, snapshot)
        })
    }

    pub fn status_root_chunks(
        &self,
        status: i32,
        phase: super::status_model::Phase,
    ) -> Result<Vec<Vec<usize>>, String> {
        let (chart, entry) = self
            .status_owner(status)
            .ok_or("status has no controller owner")?;
        let mut chunks = vec![Vec::new()];
        if entry.binding.policy == super::status_model::Policy::Owned {
            for &index in entry.binding.roots.get(&phase).into_iter().flatten() {
                chunks.last_mut().unwrap().push(index);
                if self.programs[chart].transitions[index]
                    .actions
                    .iter()
                    .any(|a| {
                        matches!(
                            a,
                            Action::Status(
                                super::status_commands::Command::TryCommonTransition { .. }
                            )
                        )
                    })
                {
                    chunks.push(Vec::new());
                }
            }
        }
        Ok(chunks)
    }

    pub fn step_status_roots(
        &self,
        state: &mut ControllerState,
        status: i32,
        phase: super::status_model::Phase,
        roots: &[usize],
        first: bool,
        snapshot: &NativeSnapshot,
    ) -> Result<ControllerStepResult, String> {
        let (chart, entry) = self
            .status_owner(status)
            .ok_or("status has no controller owner")?;
        self.step_selected(state, &[chart.to_owned()], false, |_, program, state| {
            program.step_status_roots(state, &entry.state, phase, roots, first, snapshot)
        })
    }

    /// Evaluate one ordinary hook in every chart. `OnStockStart` first resets
    /// every chart independently to its own initial state, matching actor
    /// lifecycle semantics. Conditional hooks use exact resource/status
    /// dispatch instead; a bare hook cannot prove their originating cause.
    pub fn step_hook(
        &self,
        state: &mut ControllerState,
        hook: HookName,
        snapshot: &NativeSnapshot,
    ) -> Result<ControllerStepResult, String> {
        if matches!(
            hook,
            HookName::OnDealDamage
                | HookName::OnTakeDamage
                | HookName::OnAttackShielded
                | HookName::OnShieldHit
                | HookName::OnIncomingHit
                | HookName::OnReflect
                | HookName::OnAbsorb
        ) {
            return Err("damage hooks require step_damage with an explicit pending input".into());
        }
        if hook == HookName::OnChargeTick {
            return Err("OnChargeTick must be dispatched through step_frame".into());
        }
        if hook == HookName::OnTimeout {
            return Err(
                "OnTimeout requires a chart's causal frame-expiry path or exact resource dispatch"
                    .into(),
            );
        }
        if matches!(
            hook,
            HookName::OnSpend | HookName::OnStatusEnter | HookName::OnStatusExit
        ) {
            return Err(format!(
                "{} requires exact registered source/status dispatch",
                hook.wire_name()
            ));
        }
        self.step_all(
            state,
            hook == HookName::OnStockStart,
            |program, chart_state| program.step_with_snapshot(chart_state, hook, snapshot),
        )
    }

    /// Evaluate the matching damage root in each selected chart, in chart-ID
    /// order, with one atomic state commit. The engine adapter still owns
    /// contact provenance, both actor lifetimes and native action guards.
    pub fn step_damage(
        &self,
        state: &mut ControllerState,
        input: super::native_inputs::DamageInput,
        snapshot: &NativeSnapshot,
    ) -> Result<ControllerStepResult, String> {
        let hook = match input.direction() {
            super::native_inputs::DamageDirection::Deal => HookName::OnDealDamage,
            super::native_inputs::DamageDirection::Take => HookName::OnTakeDamage,
            super::native_inputs::DamageDirection::AttackShielded => HookName::OnAttackShielded,
            super::native_inputs::DamageDirection::ShieldHit => HookName::OnShieldHit,
            super::native_inputs::DamageDirection::Incoming => HookName::OnIncomingHit,
            super::native_inputs::DamageDirection::Reflect => HookName::OnReflect,
            super::native_inputs::DamageDirection::Absorb => HookName::OnAbsorb,
        };
        let selected: Vec<_> = self
            .programs
            .iter()
            .filter(|(_, program)| program.bindings.iter().any(|binding| binding.hook == hook))
            .map(|(chart, _)| chart.clone())
            .collect();
        self.step_selected(state, &selected, false, |_, program, chart_state| {
            program.step_damage_with_snapshot(chart_state, input, snapshot)
        })
    }

    /// Deliver a proven native resource observation only to its registered
    /// consumers. The adapter still proves actor lifetime and event provenance.
    pub fn step_resource(
        &self,
        state: &mut ControllerState,
        source: ResourceSource,
        snapshot: &NativeSnapshot,
    ) -> Result<ControllerStepResult, String> {
        self.check_state(state)?;
        if self
            .programs
            .values()
            .next()
            .is_none_or(|program| program.fighter != source.fighter())
        {
            return Err(format!(
                "resource source {:?} is not valid for this controller fighter",
                source
            ));
        }
        let selected: Vec<String> = self
            .programs
            .iter()
            .filter(|(_, program)| {
                program
                    .resource_triggers
                    .iter()
                    .any(|trigger| trigger.hook == source.hook() && trigger.source == source)
            })
            .map(|(chart_id, _)| chart_id.clone())
            .collect();
        if selected.is_empty() {
            return Err(format!("no chart registers resource source {:?}", source));
        }
        self.step_selected(state, &selected, false, |_, program, chart_state| {
            program.step_with_snapshot(chart_state, source.hook(), snapshot)
        })
    }

    /// Route a current native status observation through exact allowlisted
    /// symbols. Numeric status is an adapter observation, not an authored ID.
    /// Same-value symbolic aliases are unioned once in authored binding order.
    pub fn step_status(
        &self,
        state: &mut ControllerState,
        hook: HookName,
        observed_status: i32,
        snapshot: &NativeSnapshot,
    ) -> Result<ControllerStepResult, String> {
        self.check_state(state)?;
        if !matches!(
            hook,
            HookName::OnStatusEnter | HookName::OnStatusExit | HookName::OnSpend
        ) {
            return Err("step_status requires OnStatusEnter, OnStatusExit, or OnSpend".into());
        }
        let fighter = self
            .programs
            .values()
            .next()
            .map(|program| program.fighter.as_str())
            .ok_or_else(|| "no charts can consume a status dispatch".to_owned())?;
        let mut selected = Vec::new();
        let mut transitions = BTreeMap::<String, BTreeSet<usize>>::new();
        for (chart_id, program) in &self.programs {
            for trigger in program
                .status_triggers
                .iter()
                .filter(|trigger| trigger.hook == hook)
            {
                let entry = super::registry::resolve(
                    fighter,
                    &trigger.symbol,
                    super::registry::IdentifierKind::FighterStatus,
                )
                .map_err(|error| {
                    format!("chart {chart_id:?} status {:?}: {error}", trigger.symbol)
                })?;
                let status = i32::try_from(entry.value).map_err(|_| {
                    format!(
                        "chart {chart_id:?} status {:?} exceeds runtime status range",
                        trigger.symbol
                    )
                })?;
                if status == observed_status {
                    selected.push(chart_id.clone());
                    if hook != HookName::OnSpend {
                        transitions
                            .entry(chart_id.clone())
                            .or_default()
                            .extend(trigger.transitions.iter().copied());
                    }
                }
            }
        }
        if selected.is_empty() {
            return Err(format!(
                "no chart registers {} for observed status {observed_status}",
                hook.wire_name()
            ));
        }
        selected.sort();
        selected.dedup();
        self.step_selected(state, &selected, false, |chart_id, program, chart_state| {
            if hook == HookName::OnSpend {
                program.step_with_snapshot(chart_state, hook, snapshot)
            } else {
                let indexes: Vec<usize> = transitions
                    .get(chart_id)
                    .into_iter()
                    .flat_map(|indexes| indexes.iter().copied())
                    .collect();
                program.step_exact_status_transitions_with_snapshot(
                    hook,
                    chart_state,
                    &indexes,
                    snapshot,
                )
            }
        })
    }

    /// Deliver an external zero-payload event to exactly one chart. There is
    /// deliberately no implicit same-name fanout across charts.
    pub fn step_named_event(
        &self,
        state: &mut ControllerState,
        chart_id: &str,
        name: &str,
        snapshot: &NativeSnapshot,
    ) -> Result<ControllerStepResult, String> {
        self.check_state(state)?;
        let program = self
            .programs
            .get(chart_id)
            .ok_or_else(|| format!("unknown controller chart {chart_id:?}"))?;
        let mut staged = state.charts.clone();
        let chart_state = staged
            .get_mut(chart_id)
            .ok_or_else(|| format!("state is missing controller chart {chart_id:?}"))?;
        let result = program
            .step_event_with_snapshot(chart_state, name, snapshot)
            .map_err(|error| format!("chart {chart_id:?}: {error}"))?;
        let charts = vec![ChartStepResult {
            chart_id: chart_id.to_owned(),
            result,
        }];
        let engine_actions = collect_engine_actions(&charts)?;
        state.charts = staged;
        Ok(ControllerStepResult {
            charts,
            engine_actions,
        })
    }

    /// Deliver constant typed values to exactly one selected chart. The
    /// receiver schema is checked before staging and the staged chart state is
    /// committed only after the event and its engine-action budget validate.
    pub fn step_named_event_values(
        &self,
        state: &mut ControllerState,
        chart_id: &str,
        name: &str,
        arguments: &[(&str, super::live_model::Value)],
        snapshot: &NativeSnapshot,
    ) -> Result<ControllerStepResult, String> {
        self.check_state(state)?;
        let program = self
            .programs
            .get(chart_id)
            .ok_or_else(|| format!("unknown controller chart {chart_id:?}"))?;
        program
            .validate_named_event_values(name, arguments)
            .map_err(|error| format!("chart {chart_id:?}: {error}"))?;

        let mut staged = state.charts.clone();
        let chart_state = staged
            .get_mut(chart_id)
            .ok_or_else(|| format!("state is missing controller chart {chart_id:?}"))?;
        let result = program
            .step_event_values_with_snapshot(chart_state, name, arguments, snapshot)
            .map_err(|error| format!("chart {chart_id:?}: {error}"))?;
        let charts = vec![ChartStepResult {
            chart_id: chart_id.to_owned(),
            result,
        }];
        let engine_actions = collect_engine_actions(&charts)?;
        state.charts = staged;
        Ok(ControllerStepResult {
            charts,
            engine_actions,
        })
    }

    /// Reset only one selected chart to its authored initial state.
    pub fn restart_chart(&self, state: &mut ControllerState, chart_id: &str) -> Result<(), String> {
        self.check_state(state)?;
        let program = self
            .programs
            .get(chart_id)
            .ok_or_else(|| format!("unknown controller chart {chart_id:?}"))?;
        let chart_state = state
            .charts
            .get_mut(chart_id)
            .ok_or_else(|| format!("state is missing controller chart {chart_id:?}"))?;
        *chart_state = program.initial_state();
        Ok(())
    }

    /// Build a new validated configuration and reconcile actor state. Only an
    /// exactly equal program at the same chart ID retains state; changed and
    /// added charts start fresh and removed charts are discarded.
    pub fn reconcile(
        &self,
        current: &ControllerState,
        next_programs: BTreeMap<String, LiveProgram>,
    ) -> Result<(Self, ControllerState), String> {
        self.check_state(current)?;
        let next = Self::new(next_programs)?;
        let next_state = next.reconcile_state(self, current)?;
        Ok((next, next_state))
    }

    /// Reconcile one actor's state into this already-constructed shared
    /// configuration. Calling this for multiple actors keeps all resulting
    /// states bound to the same coordinator token. Only a full program match
    /// at the same chart ID retains state.
    pub fn reconcile_state(
        &self,
        previous: &ControllerCoordinator,
        current: &ControllerState,
    ) -> Result<ControllerState, String> {
        previous.check_state(current)?;
        let mut next_state = self.initial_state();
        next_state.sampled_edges = current.sampled_edges.clone();
        for (chart_id, next_program) in &self.programs {
            let Some(old_program) = previous.programs.get(chart_id) else {
                continue;
            };
            if old_program.as_ref() == next_program.as_ref() {
                let old_state = current
                    .charts
                    .get(chart_id)
                    .ok_or_else(|| format!("state is missing controller chart {chart_id:?}"))?;
                next_state
                    .charts
                    .insert(chart_id.clone(), old_state.clone());
            }
        }
        Ok(next_state)
    }

    fn check_state(&self, state: &ControllerState) -> Result<(), String> {
        if !Arc::ptr_eq(&self.state_token, &state.token) {
            return Err("controller state belongs to a different configuration".into());
        }
        if state.charts.len() != self.programs.len()
            || self
                .programs
                .keys()
                .any(|chart_id| !state.charts.contains_key(chart_id))
        {
            return Err("controller state chart set does not match its configuration".into());
        }
        Ok(())
    }

    fn step_all<F>(
        &self,
        state: &mut ControllerState,
        reset_before_evaluation: bool,
        mut evaluate: F,
    ) -> Result<ControllerStepResult, String>
    where
        F: FnMut(&LiveProgram, &mut LiveState) -> Result<StepResult, EvaluationError>,
    {
        self.check_state(state)?;
        let selected: Vec<String> = self.programs.keys().cloned().collect();
        if selected.is_empty() {
            return Ok(ControllerStepResult {
                charts: Vec::new(),
                engine_actions: Vec::new(),
            });
        }
        self.step_selected(
            state,
            &selected,
            reset_before_evaluation,
            |_, program, chart_state| evaluate(program, chart_state),
        )
    }

    fn step_selected<F>(
        &self,
        state: &mut ControllerState,
        selected: &[String],
        reset_before_evaluation: bool,
        mut evaluate: F,
    ) -> Result<ControllerStepResult, String>
    where
        F: FnMut(&str, &LiveProgram, &mut LiveState) -> Result<StepResult, EvaluationError>,
    {
        self.check_state(state)?;
        if selected.is_empty() {
            return Ok(ControllerStepResult {
                charts: Vec::new(),
                engine_actions: Vec::new(),
            });
        }
        if selected.len() > MAX_CHARTS || selected.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err("selected charts must be unique and in stable chart-ID order".into());
        }
        let mut staged = state.charts.clone();
        if reset_before_evaluation {
            for chart_id in selected {
                let program = self
                    .programs
                    .get(chart_id)
                    .ok_or_else(|| format!("unknown controller chart {chart_id:?}"))?;
                staged.insert(chart_id.clone(), program.initial_state());
            }
        }
        let mut charts = Vec::with_capacity(selected.len());
        let mut action_count = 0usize;
        for chart_id in selected {
            let program = self
                .programs
                .get(chart_id)
                .ok_or_else(|| format!("unknown controller chart {chart_id:?}"))?;
            let chart_state = staged
                .get_mut(chart_id)
                .ok_or_else(|| format!("state is missing controller chart {chart_id:?}"))?;
            let result = evaluate(chart_id, program, chart_state)
                .map_err(|error| format!("chart {chart_id:?}: {error}"))?;
            action_count =
                checked_add(action_count, result.engine_actions.len(), "engine actions")?;
            ensure_budget(
                "root-step engine action",
                action_count,
                MAX_ENGINE_ACTIONS_PER_STEP,
            )?;
            charts.push(ChartStepResult {
                chart_id: chart_id.clone(),
                result,
            });
        }
        let engine_actions = collect_engine_actions(&charts)?;
        state.charts = staged;
        if reset_before_evaluation {
            state.sampled_edges.clear();
        }
        Ok(ControllerStepResult {
            charts,
            engine_actions,
        })
    }
}

impl ControllerState {
    pub fn chart(&self, chart_id: &str) -> Option<&LiveState> {
        self.charts.get(chart_id)
    }

    pub fn chart_count(&self) -> usize {
        self.charts.len()
    }
}

/// Reuse `LiveProgram::validate` as the authority for state-name membership,
/// variable types, bounded finite values, and timer ranges. The exact key-set
/// check comes first so the probe cannot hide missing or extra values by
/// rebuilding its variable table from the state being checked.
fn validate_live_state(program: &LiveProgram, state: &LiveState) -> Result<(), String> {
    if state.values.len() != program.vars.len()
        || program
            .vars
            .iter()
            .any(|var| !state.values.contains_key(&var.name))
    {
        return Err("legacy main state variable set does not match its program".into());
    }

    let mut probe = program.clone();
    if !program.states.contains(&state.state) && !program.states.is_empty() {
        return Err("runtime state is not declared".into());
    }
    for var in &mut probe.vars {
        var.initial = *state
            .values
            .get(&var.name)
            .ok_or_else(|| format!("legacy main state is missing variable {:?}", var.name))?;
    }
    // Empty models have no declared state list to check against. Their only
    // meaningful state is the program's initial state.
    if program.states.is_empty() && state.state != program.initial {
        return Err("legacy main state differs from empty program initial state".into());
    }
    probe.validate().map_err(|errors| {
        errors
            .into_iter()
            .map(|error| format!("{}: {}", error.item, error.reason))
            .collect::<Vec<_>>()
            .join("; ")
    })
}

fn validate_program_set(programs: &BTreeMap<String, LiveProgram>) -> Result<(), String> {
    if programs.len() > MAX_CHARTS {
        return Err(format!(
            "controller chart count exceeds the limit of {MAX_CHARTS}"
        ));
    }

    super::status_model::manifest(programs.values().cloned())?;
    let mut fighter: Option<&str> = None;
    let mut ids = BTreeSet::new();
    let mut variables = 0usize;
    let mut transitions = 0usize;
    let mut event_bindings = 0usize;
    let mut native_inputs = BTreeSet::<NativeInput>::new();
    let mut authored_actions = 0usize;

    for (chart_id, program) in programs {
        if chart_id.trim().is_empty()
            || chart_id.len() > super::live_model::MAX_NAME_BYTES
            || chart_id.as_bytes().contains(&0)
        {
            return Err(format!("invalid controller chart ID {chart_id:?}"));
        }
        if let Some(expected) = fighter {
            if program.fighter != expected {
                return Err(format!(
                    "chart {chart_id:?} targets fighter {:?}, expected {expected:?}",
                    program.fighter
                ));
            }
        } else {
            fighter = Some(&program.fighter);
        }
        if !ids.insert(program.id.as_str()) {
            return Err(format!(
                "chart {chart_id:?} duplicates program ID {:?}",
                program.id
            ));
        }
        let errors = program.validate().err().unwrap_or_default();
        if !errors.is_empty() {
            return Err(format!(
                "chart {chart_id:?} program {:?} is invalid: {}",
                program.id,
                errors
                    .into_iter()
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }

        variables = checked_add(variables, program.vars.len(), "variables")?;
        transitions = checked_add(transitions, program.transitions.len(), "transitions")?;
        event_bindings = checked_add(
            event_bindings,
            program.event_bindings.len(),
            "event bindings",
        )?;
        for input in &program.native_inputs {
            native_inputs.insert(input.clone());
        }
        for transition in &program.transitions {
            authored_actions = checked_add(
                authored_actions,
                transition.actions.len(),
                "authored actions",
            )?;
        }
    }

    ensure_budget("variables", variables, MAX_AGGREGATE_VARIABLES)?;
    ensure_budget("transitions", transitions, MAX_AGGREGATE_TRANSITIONS)?;
    ensure_budget(
        "event bindings",
        event_bindings,
        MAX_AGGREGATE_EVENT_BINDINGS,
    )?;
    ensure_budget(
        "union native inputs",
        native_inputs.len(),
        MAX_UNION_NATIVE_INPUTS.min(MAX_NATIVE_INPUTS),
    )?;
    ensure_budget(
        "authored actions",
        authored_actions,
        MAX_AGGREGATE_AUTHORED_ACTIONS,
    )?;
    debug_assert!(variables <= MAX_CHARTS * MAX_VARIABLES);
    Ok(())
}

fn checked_add(total: usize, next: usize, label: &str) -> Result<usize, String> {
    total
        .checked_add(next)
        .ok_or_else(|| format!("aggregate {label} count overflowed"))
}

fn ensure_budget(label: &str, count: usize, limit: usize) -> Result<(), String> {
    if count > limit {
        Err(format!(
            "aggregate {label} count {count} exceeds the limit of {limit}"
        ))
    } else {
        Ok(())
    }
}

fn collect_engine_actions(charts: &[ChartStepResult]) -> Result<Vec<ChartEngineAction>, String> {
    let count = charts.iter().try_fold(0usize, |count, chart| {
        checked_add(count, chart.result.engine_actions.len(), "engine actions")
    })?;
    ensure_budget(
        "root-step engine action",
        count,
        MAX_ENGINE_ACTIONS_PER_STEP,
    )?;
    let mut actions = Vec::with_capacity(count);
    for chart in charts {
        actions.extend(chart.result.engine_actions.iter().cloned().map(|action| {
            ChartEngineAction {
                chart_id: chart.chart_id.clone(),
                action,
            }
        }));
    }
    Ok(actions)
}

#[cfg(test)]
mod tests {
    use super::super::live_model::{
        CmpOp, EventBinding, Expr, HookBinding, Predicate, Transition, Value, VarDef, VarKind,
    };
    use super::*;

    #[test]
    fn empty_dispatch_is_noop_but_foreign_state_is_rejected() {
        let coordinator = ControllerCoordinator::new(BTreeMap::new()).unwrap();
        let mut state = coordinator.initial_state();
        let result = coordinator
            .step_frame(&mut state, &NativeSnapshot::default())
            .unwrap();
        assert!(result.charts.is_empty());
        assert!(result.engine_actions.is_empty());
        let other = ControllerCoordinator::new(BTreeMap::new()).unwrap();
        assert!(other
            .step_frame(&mut state, &NativeSnapshot::default())
            .is_err());
    }

    #[test]
    fn switch_after_carry_commits_variables_and_emits_native_actions_once() {
        let mut p = program("switch", 90);
        p.vars[1].keep_across_form_switch = true;
        p.transitions.truncate(1);
        p.transitions[0].actions.push(Action::AddDamage {
            amount: Expr::Const(Value::Float(10.0)),
        });
        p.bindings = vec![HookBinding {
            hook: HookName::OnSwitch,
            article: None,
            transitions: vec![0],
        }];
        p.event_bindings.clear();
        let coordinator =
            ControllerCoordinator::new(BTreeMap::from([("shared".into(), p)])).unwrap();
        let mut outgoing = coordinator.initial_state();
        outgoing
            .charts
            .get_mut("shared")
            .unwrap()
            .values
            .insert("count".into(), Value::Int(7));
        let mut incoming = coordinator.initial_state();
        coordinator
            .carry_form_variables(&mut incoming, &coordinator, &outgoing)
            .unwrap();
        let result = coordinator
            .step_hook(
                &mut incoming,
                HookName::OnSwitch,
                &NativeSnapshot::default(),
            )
            .unwrap();
        assert_eq!(incoming.charts["shared"].values["count"], Value::Int(8));
        assert_eq!(result.engine_actions.len(), 1);
        assert!(matches!(
            result.engine_actions[0].action,
            Action::AddDamage { .. }
        ));
        let frame = coordinator
            .step_frame(&mut incoming, &NativeSnapshot::default())
            .unwrap();
        assert!(frame.engine_actions.is_empty());
        assert_eq!(incoming.charts["shared"].values["count"], Value::Int(8));
    }

    #[test]
    fn form_transfer_requires_both_opt_ins_chart_name_and_type() {
        let mut p = program("out", 90);
        p.vars[1].keep_across_form_switch = true;
        let outgoing =
            ControllerCoordinator::new(BTreeMap::from([("shared".into(), p.clone())])).unwrap();
        let mut old = outgoing.initial_state();
        old.charts
            .get_mut("shared")
            .unwrap()
            .values
            .insert("count".into(), Value::Int(7));
        for (chart, opt_in, kind, expected) in [
            ("shared", true, VarKind::Int, Value::Int(7)),
            ("shared", false, VarKind::Int, Value::Int(0)),
            ("other", true, VarKind::Int, Value::Int(0)),
        ] {
            let mut incoming_program = p.clone();
            incoming_program.id = "in".into();
            incoming_program.vars[1].keep_across_form_switch = opt_in;
            incoming_program.vars[1].kind = kind;
            let incoming =
                ControllerCoordinator::new(BTreeMap::from([(chart.into(), incoming_program)]))
                    .unwrap();
            let mut state = incoming.initial_state();
            incoming
                .carry_form_variables(&mut state, &outgoing, &old)
                .unwrap();
            assert_eq!(state.charts[chart].values["count"], expected);
            assert_eq!(state.charts[chart].values["timer"], Value::Int(90));
        }
        p.vars[1].keep_across_form_switch = false;
        let unshared = ControllerCoordinator::new(BTreeMap::from([("shared".into(), p)])).unwrap();
        let mut incoming_state = outgoing.initial_state();
        let mut unshared_state = unshared.initial_state();
        unshared_state
            .charts
            .get_mut("shared")
            .unwrap()
            .values
            .insert("count".into(), Value::Int(9));
        outgoing
            .carry_form_variables(&mut incoming_state, &unshared, &unshared_state)
            .unwrap();
        assert_eq!(
            incoming_state.charts["shared"].values["count"],
            Value::Int(0)
        );
        let mut different = program("float", 90);
        different.transitions.truncate(1);
        different.transitions[0].actions.clear();
        different.transitions[0].guard = always();
        different.bindings.truncate(1);
        different.event_bindings.clear();
        different.vars[1].kind = VarKind::Float;
        different.vars[1].initial = Value::Float(0.0);
        different.vars[1].keep_across_form_switch = true;
        let different =
            ControllerCoordinator::new(BTreeMap::from([("shared".into(), different)])).unwrap();
        let mut different_state = different.initial_state();
        different
            .carry_form_variables(&mut different_state, &outgoing, &old)
            .unwrap();
        assert_eq!(
            different_state.charts["shared"].values["count"],
            Value::Float(0.0)
        );
        let mut fresh = unshared.initial_state();
        assert!(outgoing
            .carry_form_variables(&mut fresh, &outgoing, &old)
            .is_err());
    }

    fn always() -> Predicate {
        Predicate::Compare {
            op: CmpOp::Eq,
            left: Expr::Const(Value::Int(1)),
            right: Expr::Const(Value::Int(1)),
        }
    }

    fn program(id: &str, timer: i32) -> LiveProgram {
        LiveProgram {
            costumes: Vec::new(),
            custom_statuses: Default::default(),
            status_bindings: Vec::new(),
            version: super::super::live_model::MODEL_VERSION,
            id: id.into(),
            fighter: "cloud".into(),
            initial: "idle".into(),
            states: vec!["idle".into()],
            vars: vec![
                VarDef {
                    keep_across_form_switch: false,
                    name: "timer".into(),
                    kind: VarKind::Timer,
                    initial: Value::Int(timer),
                },
                VarDef {
                    keep_across_form_switch: false,
                    name: "count".into(),
                    kind: VarKind::Int,
                    initial: Value::Int(0),
                },
            ],
            native_inputs: vec![],
            transitions: vec![
                Transition {
                    from: "idle".into(),
                    to: "idle".into(),
                    guard: always(),
                    on_frame: true,
                    actions: vec![Action::Add {
                        var: "count".into(),
                        value: Expr::Const(Value::Int(1)),
                    }],
                },
                Transition {
                    from: "idle".into(),
                    to: "idle".into(),
                    guard: Predicate::Expired("timer".into()),
                    on_frame: true,
                    actions: vec![Action::Add {
                        var: "count".into(),
                        value: Expr::Const(Value::Int(10)),
                    }],
                },
                Transition {
                    from: "idle".into(),
                    to: "idle".into(),
                    guard: always(),
                    on_frame: true,
                    actions: vec![Action::Add {
                        var: "count".into(),
                        value: Expr::Const(Value::Int(100)),
                    }],
                },
                Transition {
                    from: "idle".into(),
                    to: "idle".into(),
                    guard: always(),
                    on_frame: true,
                    actions: vec![Action::Add {
                        var: "count".into(),
                        value: Expr::Const(Value::Int(1)),
                    }],
                },
                Transition {
                    from: "idle".into(),
                    to: "idle".into(),
                    guard: always(),
                    on_frame: true,
                    actions: vec![Action::Add {
                        var: "count".into(),
                        value: Expr::Const(Value::Int(1_000)),
                    }],
                },
            ],
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
                    hook: HookName::OnArticleSpawn,
                    article: None,
                    transitions: vec![3],
                },
                HookBinding {
                    hook: HookName::OnStockStart,
                    article: None,
                    transitions: vec![4],
                },
            ],
            event_bindings: vec![EventBinding {
                name: "pulse".into(),
                transitions: vec![2],
                fields: vec![],
            }],
            status_triggers: vec![],
            resource_triggers: vec![],
        }
    }

    fn coordinator(entries: &[(&str, LiveProgram)]) -> ControllerCoordinator {
        ControllerCoordinator::new(
            entries
                .iter()
                .map(|(id, program)| ((*id).to_owned(), program.clone()))
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn failed_chart_evaluation_rolls_back_all_charts() {
        let mut first = program("cloud-first", 5);
        first.native_inputs.push(NativeInput::CloudLimitGauge);
        first.transitions[0].guard = Predicate::Compare {
            op: CmpOp::Gte,
            left: Expr::NativeFloat(NativeInput::CloudLimitGauge),
            right: Expr::Const(Value::Float(0.0)),
        };
        let runtime = coordinator(&[("a", program("cloud-a", 5)), ("z", first)]);
        let mut state = runtime.initial_state();
        let before_a = state.chart("a").unwrap().snapshot();
        let before_z = state.chart("z").unwrap().snapshot();
        assert!(runtime
            .step_frame(&mut state, &NativeSnapshot::empty())
            .unwrap_err()
            .contains("chart \"z\""));
        assert_eq!(state.chart("a").unwrap().snapshot(), before_a);
        assert_eq!(state.chart("z").unwrap().snapshot(), before_z);
    }

    #[test]
    fn root_engine_action_limit_rolls_back_every_chart() {
        let earlier = program("cloud-earlier", 5);
        let mut overloaded = program("cloud-overloaded", 5);
        overloaded.transitions.clear();
        overloaded.event_bindings.clear();
        overloaded.bindings = vec![HookBinding {
            hook: HookName::OnChargeTick,
            article: None,
            transitions: vec![],
        }];
        let mut first_actions = vec![Action::Set {
            var: "count".into(),
            value: Expr::Const(Value::Int(99)),
        }];
        first_actions.extend((0..23).map(|_| Action::GenerateArticle {
            symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
        }));
        overloaded.transitions.push(Transition {
            from: "idle".into(),
            to: "idle".into(),
            guard: always(),
            on_frame: true,
            actions: first_actions,
        });
        for _ in 0..10 {
            overloaded.transitions.push(Transition {
                from: "idle".into(),
                to: "idle".into(),
                guard: always(),
                on_frame: true,
                actions: (0..24)
                    .map(|_| Action::GenerateArticle {
                        symbol: "FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE".into(),
                    })
                    .collect(),
            });
        }
        overloaded.bindings[0].transitions = (0..overloaded.transitions.len()).collect();
        let runtime = coordinator(&[("a", earlier), ("z", overloaded)]);
        let mut state = runtime.initial_state();
        let before_a = state.chart("a").unwrap().snapshot();
        let before_z = state.chart("z").unwrap().snapshot();
        let error = runtime
            .step_frame(&mut state, &NativeSnapshot::empty())
            .unwrap_err();
        assert!(error.contains("engine action"));
        assert_eq!(state.chart("a").unwrap().snapshot(), before_a);
        assert_eq!(state.chart("z").unwrap().snapshot(), before_z);
    }
}
