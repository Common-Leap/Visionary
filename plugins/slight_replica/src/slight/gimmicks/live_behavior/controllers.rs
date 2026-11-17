//! Native V5 live and compiled-controller adapter. The live request versions
//! and compiled baseline share one mutex, actor-slot pool and dispatch tickets.

use std::io::{self, Write};

use super::super::controller_protocol as protocol;
use super::super::controller_runtime::{ControllerCoordinator, ControllerState};
use super::*;

mod form_switch;
pub(super) use form_switch::observe_form;
pub(super) use form_switch::{clear_form_entry, clear_forms};

const MAX_CONTROLLER_TELEMETRY_BYTES: usize = 512 * 1024;

struct BoundedTelemetry(Vec<u8>);

impl Write for BoundedTelemetry {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_CONTROLLER_TELEMETRY_BYTES {
            return Err(io::Error::other(
                "controller telemetry payload cap exceeded",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) struct ObjectState {
    pub(super) identity: Identity,
    pub(super) fighter: String,
    state: ControllerState,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ControllerStateRow {
    #[serde(flatten)]
    pub actor: StateRow,
    pub chart_id: String,
    pub program_id: String,
    pub stock_generation: u32,
}

fn same_programs(left: &ControllerCoordinator, right: &ControllerCoordinator) -> bool {
    left.chart_ids().eq(right.chart_ids())
        && left
            .chart_ids()
            .all(|id| left.program(id) == right.program(id))
}

fn same_bundle(
    left: &BTreeMap<String, Arc<ControllerCoordinator>>,
    right: &BTreeMap<String, Arc<ControllerCoordinator>>,
) -> bool {
    left.len() == right.len()
        && left.iter().all(|(fighter, old)| {
            right
                .get(fighter)
                .is_some_and(|next| same_programs(old, next))
        })
}

/// Install one immutable package baseline. Revision zero is a neutral codec
/// value only; it is discarded and never enters the live ACK/revision state.
pub(super) fn install_compiled_baseline(raw: &str) -> Result<(), String> {
    let request = protocol::decode_request(raw)?;
    if request.revision != 0 {
        return Err("compiled controller baseline must use neutral revision 0".into());
    }
    if !request.disabled_fighters.is_empty() {
        return Err("compiled controller baseline cannot contain live disabled owners".into());
    }
    if request.charts.is_empty() {
        return Err("compiled controller baseline must contain at least one chart".into());
    }
    for chart in &request.charts {
        validate_engine_symbols(&chart.program)
            .map_err(|error| format!("chart {:?}: {error}", chart.chart_id))?;
    }
    let manifest =
        super::super::status_model::manifest(request.charts.iter().map(|c| c.program.clone()))?;
    let candidate: BTreeMap<String, Arc<ControllerCoordinator>> = request
        .coordinators()?
        .into_iter()
        .map(|(fighter, coordinator)| (fighter, Arc::new(coordinator)))
        .collect();
    let mut runtime = RUNTIME.lock();
    if runtime.compiled_controllers.is_empty() {
        super::super::damage_control::configuration_changed();
        super::super::damage_feed::configuration_changed();
        runtime.custom_statuses = request
            .charts
            .iter()
            .flat_map(|c| {
                c.program
                    .custom_statuses
                    .iter()
                    .map(|(key, slot)| ((c.program.fighter.clone(), key.clone()), *slot))
            })
            .collect();
        runtime.status_manifest = manifest;
        runtime.compiled_controllers = candidate;
        refresh_cloud_resource_demand(&runtime);
        return Ok(());
    }
    if same_bundle(&runtime.compiled_controllers, &candidate) {
        return Ok(());
    }
    Err("a different compiled controller baseline is already installed".into())
}

pub(super) fn compiled_owner_installed(fighter: &str) -> bool {
    RUNTIME.lock().compiled_controllers.contains_key(fighter)
}

pub(super) fn compiled_owner_effective(fighter: &str) -> bool {
    effective_compiled_owner(&RUNTIME.lock(), fighter)
}

fn effective_compiled_owner(runtime: &Runtime, fighter: &str) -> bool {
    !runtime.controllers.contains_key(fighter)
        && !runtime.disabled_fighters.contains(fighter)
        && runtime.compiled_controllers.contains_key(fighter)
}

#[cfg(test)]
pub(super) fn test_effective_source(fighter: &str) -> Option<&'static str> {
    let runtime = RUNTIME.lock();
    if runtime.controllers.contains_key(fighter) {
        Some("live")
    } else if runtime.disabled_fighters.contains(fighter) {
        Some("disabled")
    } else if runtime.compiled_controllers.contains_key(fighter) {
        Some("compiled")
    } else {
        None
    }
}

#[cfg(test)]
pub(super) fn test_insert_hidden_baseline_state(boid: u32, fighter: &str) -> bool {
    let Some(identity) = Identity::from_live_fighter(boid, fighter) else {
        return false;
    };
    let mut runtime = RUNTIME.lock();
    let Some(coordinator) = runtime.compiled_controllers.get(fighter).cloned() else {
        return false;
    };
    let state = coordinator.initial_state();
    runtime.compiled_controller_objects.insert(
        boid,
        ObjectState {
            identity,
            fighter: fighter.into(),
            state,
        },
    );
    true
}

pub(super) fn replace(raw: &str) -> Ack {
    let request = match protocol::decode_request(raw) {
        Ok(request) => request,
        Err(error) => return Ack::rejected(request_revision(raw), error),
    };
    let revision = request.revision;
    for chart in &request.charts {
        if let Err(error) = validate_engine_symbols(&chart.program) {
            return Ack::rejected(revision, format!("chart {:?}: {error}", chart.chart_id));
        }
    }
    let mut candidate: BTreeMap<String, Arc<ControllerCoordinator>> = match request.coordinators() {
        Ok(configurations) => configurations
            .into_iter()
            .map(|(fighter, coordinator)| (fighter, Arc::new(coordinator)))
            .collect(),
        Err(error) => return Ack::rejected(revision, error),
    };
    let manifest = match super::super::status_model::manifest(
        request.charts.iter().map(|c| c.program.clone()),
    ) {
        Ok(m) => m,
        Err(e) => return Ack::rejected(revision, e),
    };
    let encoded = match protocol::encode_request(&request) {
        Ok(raw) => raw,
        Err(e) => return Ack::rejected(revision, e),
    };
    let disabled: BTreeSet<_> = request.disabled_fighters.into_iter().collect();
    let mut runtime = RUNTIME.lock();
    if request.charts.iter().any(|c| {
        c.program.custom_statuses.iter().any(|(key, slot)| {
            runtime
                .custom_statuses
                .get(&(c.program.fighter.clone(), key.clone()))
                != Some(slot)
        })
    }) {
        return Ack::rejected(
            revision,
            "custom status allocation changed; export plugin and reload",
        );
    }
    if !manifest.is_subset(&runtime.status_manifest) {
        return Ack::rejected(
            revision,
            "status installation changed; export plugin and reload",
        );
    }
    if let Some((pending, raw, _)) = &runtime.pending_status_update {
        if revision < *pending {
            return Ack::rejected(revision, "stale staged controller revision");
        }
        if revision == *pending {
            return if raw == &encoded {
                Ack::accepted(revision, "controller revision already staged")
            } else {
                Ack::rejected(revision, "same staged revision carries different content")
            };
        }
    }
    if runtime.has_snapshot && revision < runtime.revision {
        return Ack::rejected(revision, "stale controller bundle revision");
    }
    let same = runtime.disabled_fighters == disabled
        && runtime.controllers.len() == candidate.len()
        && runtime.controllers.iter().all(|(fighter, old)| {
            candidate
                .get(fighter)
                .is_some_and(|next| same_programs(old, next))
        });
    if runtime.has_snapshot && revision == runtime.revision {
        return if same {
            Ack::accepted(revision, "controller bundle already active")
        } else {
            Ack::rejected(revision, "same behavior revision carries different content")
        };
    }

    if !runtime.status_leases.is_empty() {
        runtime.pending_status_update = Some((revision, encoded, disabled));
        return Ack::accepted(revision, "controller revision staged until status Exit");
    }
    let mut changed = BTreeSet::new();
    for (fighter, old) in &runtime.controllers {
        match candidate.get_mut(fighter) {
            Some(next) if same_programs(old, next) => *next = Arc::clone(old),
            _ => {
                changed.insert(fighter.clone());
            }
        }
    }
    changed.extend(
        candidate
            .keys()
            .filter(|fighter| !runtime.controllers.contains_key(*fighter))
            .cloned(),
    );
    changed.extend(
        runtime
            .disabled_fighters
            .symmetric_difference(&disabled)
            .cloned(),
    );

    // Reconcile EVERY retained actor into the same new configuration for its
    // fighter, before committing any part of the candidate snapshot.
    let mut objects = BTreeMap::new();
    for (boid, object) in &runtime.controller_objects {
        let (Some(next), Some(previous)) = (
            candidate.get(&object.fighter),
            runtime.controllers.get(&object.fighter),
        ) else {
            continue;
        };
        let state = match next.reconcile_state(previous, &object.state) {
            Ok(state) => state,
            Err(error) => {
                return Ack::rejected(revision, format!("actor {boid} reconciliation: {error}"))
            }
        };
        objects.insert(
            *boid,
            ObjectState {
                identity: object.identity,
                fighter: object.fighter.clone(),
                state,
            },
        );
    }
    super::super::damage_control::configuration_changed();
    super::super::damage_feed::configuration_changed();
    runtime.controllers = candidate;
    runtime.controller_objects = objects;
    runtime.disabled_fighters = disabled;
    let masked: BTreeSet<_> = runtime
        .controllers
        .keys()
        .chain(runtime.disabled_fighters.iter())
        .cloned()
        .collect();
    runtime
        .compiled_controller_objects
        .retain(|_, object| !masked.contains(&object.fighter));
    runtime.has_snapshot = true;
    runtime.revision = revision;
    runtime
        .pending_dispatch
        .retain(|_, pending| !changed.contains(&pending.fighter));
    refresh_cloud_resource_demand(&runtime);
    Ack::accepted(
        revision,
        format!(
            "installed full controller bundle; {} fighter owners, {} explicit disables",
            runtime.controllers.len(),
            runtime.disabled_fighters.len()
        ),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DispatchSource {
    Live { revision: u64 },
    Compiled,
    Status { epoch: u64 },
}

struct DispatchContext {
    boid: u32,
    fighter: String,
    identity: Identity,
    source: DispatchSource,
    coordinator: Arc<ControllerCoordinator>,
    ticket: ActorDispatchTicket,
    contact: Option<super::super::damage_dispatch::Event>,
}

impl DispatchContext {
    fn matches_runtime(&self, runtime: &Runtime) -> bool {
        let configuration_current = match self.source {
            DispatchSource::Live { revision } => {
                runtime.revision == revision
                    && runtime
                        .controllers
                        .get(&self.fighter)
                        .is_some_and(|active| Arc::ptr_eq(active, &self.coordinator))
            }
            DispatchSource::Status { epoch } => {
                runtime.status_leases.get(&self.boid).is_some_and(|lease| {
                    lease.epoch == epoch
                        && lease.object.identity == self.identity
                        && Arc::ptr_eq(&lease.coordinator, &self.coordinator)
                })
            }
            DispatchSource::Compiled => {
                effective_compiled_owner(runtime, &self.fighter)
                    && runtime
                        .compiled_controllers
                        .get(&self.fighter)
                        .is_some_and(|active| Arc::ptr_eq(active, &self.coordinator))
            }
        };
        configuration_current
            && runtime
                .pending_dispatch
                .get(&self.boid)
                .is_some_and(|pending| {
                    pending.ticket == self.ticket.ticket && pending.fighter == self.fighter
                })
    }

    fn is_current(&self) -> bool {
        Identity::from_live_fighter(self.boid, &self.fighter) == Some(self.identity)
            && super::super::damage_context::is_current(self.contact, self.boid, self.identity)
            && self.matches_runtime(&RUNTIME.lock())
    }

    fn commit(&self, state: ControllerState) -> Result<(), String> {
        if !self.is_current() {
            return Err("controller dispatch context changed before state commit".into());
        }
        let mut runtime = RUNTIME.lock();
        if !self.matches_runtime(&runtime) {
            return Err("controller dispatch context changed before state commit".into());
        }
        let object = ObjectState {
            identity: self.identity,
            fighter: self.fighter.clone(),
            state,
        };
        match self.source {
            DispatchSource::Status { .. } => {
                runtime
                    .status_leases
                    .get_mut(&self.boid)
                    .ok_or("status lease ended before commit")?
                    .object = object;
            }
            DispatchSource::Live { .. } => {
                runtime.controller_objects.insert(self.boid, object);
            }
            DispatchSource::Compiled => {
                runtime
                    .compiled_controller_objects
                    .insert(self.boid, object);
            }
        }
        Ok(())
    }
}

fn begin(
    boid: u32,
    fighter: &str,
    coordinator: Arc<ControllerCoordinator>,
    source: DispatchSource,
    contact: Option<super::super::damage_dispatch::Event>,
) -> Result<(DispatchContext, ControllerState), String> {
    let identity = Identity::from_live_fighter(boid, fighter).ok_or_else(|| {
        format!("battle object {boid} is not the exact registered {fighter} fighter identity")
    })?;
    super::super::damage_context::require_current(contact, boid, identity)?;
    let mut runtime = RUNTIME.lock();
    let source_is_current = match source {
        DispatchSource::Status { epoch } => runtime.status_leases.get(&boid).is_some_and(|lease| {
            lease.epoch == epoch
                && lease.object.identity == identity
                && Arc::ptr_eq(&lease.coordinator, &coordinator)
        }),
        DispatchSource::Live { revision } => {
            runtime.revision == revision
                && runtime
                    .controllers
                    .get(fighter)
                    .is_some_and(|active| Arc::ptr_eq(active, &coordinator))
        }
        DispatchSource::Compiled => {
            effective_compiled_owner(&runtime, fighter)
                && runtime
                    .compiled_controllers
                    .get(fighter)
                    .is_some_and(|active| Arc::ptr_eq(active, &coordinator))
        }
    };
    if !source_is_current {
        return Err("controller configuration changed before dispatch".into());
    }
    if !(0..8).contains(&identity.entry_id) {
        return Err("controller actor entry is outside the eight fighter slots".into());
    }
    if !actor_slot_available(&runtime, boid) {
        return Err(ACTOR_CAPACITY_ERROR.into());
    }
    let object = match source {
        DispatchSource::Live { .. } => runtime.controller_objects.get(&boid),
        DispatchSource::Compiled => runtime.compiled_controller_objects.get(&boid),
        DispatchSource::Status { .. } => {
            runtime.status_leases.get(&boid).map(|lease| &lease.object)
        }
    };
    let state = object
        .filter(|object| object.identity == identity && object.fighter == fighter)
        .map(|object| object.state.clone())
        .unwrap_or_else(|| coordinator.initial_state());
    runtime.next_dispatch_ticket = runtime.next_dispatch_ticket.wrapping_add(1).max(1);
    let ticket = runtime.next_dispatch_ticket;
    runtime.pending_dispatch.insert(
        boid,
        PendingDispatch {
            ticket,
            fighter: fighter.into(),
        },
    );
    Ok((
        DispatchContext {
            boid,
            fighter: fighter.into(),
            identity,
            source,
            coordinator,
            contact,
            ticket: ActorDispatchTicket {
                boid,
                ticket,
                fighter: fighter.into(),
            },
        },
        state,
    ))
}

fn active(fighter: &str) -> Result<(Arc<ControllerCoordinator>, DispatchSource), String> {
    let runtime = RUNTIME.lock();
    if let Some(coordinator) = runtime.controllers.get(fighter) {
        return Ok((
            Arc::clone(coordinator),
            DispatchSource::Live {
                revision: runtime.revision,
            },
        ));
    }
    if runtime.disabled_fighters.contains(fighter) {
        return Err(format!(
            "live behavior overrides compiled controllers for {fighter:?}"
        ));
    }
    runtime
        .compiled_controllers
        .get(fighter)
        .cloned()
        .map(|coordinator| (coordinator, DispatchSource::Compiled))
        .ok_or_else(|| format!("no live or compiled controller bundle for {fighter:?}"))
}

fn preflight_actions(
    context: &DispatchContext,
    actions: &[super::super::controller_runtime::ChartEngineAction],
) -> Result<(), String> {
    for tagged in actions {
        if !context.is_current() {
            return Err("controller dispatch changed before resource preflight".into());
        }
        let program = context
            .coordinator
            .program(&tagged.chart_id)
            .ok_or("native action references an unknown controller chart")?;
        super::super::operations::preflight_behavior_operation(
            &program.fighter,
            context.boid,
            &tagged.action,
        )?;
        preflight_engine_actions(
            context.boid,
            context.identity,
            std::slice::from_ref(&tagged.action),
        )?;
    }
    Ok(())
}

// Acceptance is the single state commit. No engine operation is issued
// before it; native delivery errors after acceptance are diagnostics, never a
// rejected dispatch that a caller could retry with already-applied effects.
fn accept_dispatch(
    context: &DispatchContext,
    state: ControllerState,
    actions: &[super::super::controller_runtime::ChartEngineAction],
) -> Result<(), String> {
    preflight_actions(context, actions)?;
    context.commit(state)?;
    if let Err(error) = execute_preflighted_actions(context, actions) {
        crate::slight::diag::note(format!(
            "accepted controller dispatch {}/{}: native delivery stopped: {error}",
            context.fighter, context.boid,
        ));
    }
    Ok(())
}

fn execute_preflighted_actions(
    context: &DispatchContext,
    actions: &[super::super::controller_runtime::ChartEngineAction],
) -> Result<(), String> {
    for tagged in actions {
        if !context.is_current() {
            return Err("controller dispatch context changed before native action".into());
        }
        let program = context
            .coordinator
            .program(&tagged.chart_id)
            .ok_or_else(|| "native action references an unknown controller chart".to_owned())?;
        execute_engine_action(program, context.boid, context.identity, &tagged.action)?;
        if !context.is_current() {
            return Err("controller dispatch context changed during native action".into());
        }
    }
    Ok(())
}

/// Read the effective chart without allocating an actor slot or dispatching events.
/// A live owner (including one missing this chart) masks the packaged baseline.
pub fn read_controller_value(
    boid: u32,
    fighter: &str,
    chart_id: &str,
    var: &str,
) -> Option<model::Value> {
    let identity = Identity::from_live_fighter(boid, fighter)?;
    let runtime = RUNTIME.lock();
    if let Some(lease) = runtime
        .status_leases
        .get(&boid)
        .filter(|l| l.object.identity == identity && l.object.fighter == fighter)
    {
        return lease.object.state.chart(chart_id)?.values.get(var).copied();
    }
    let (coordinator, objects) = if let Some(coordinator) = runtime.controllers.get(fighter) {
        (coordinator, &runtime.controller_objects)
    } else if effective_compiled_owner(&runtime, fighter) {
        (
            runtime.compiled_controllers.get(fighter)?,
            &runtime.compiled_controller_objects,
        )
    } else {
        return None;
    };
    let definition = coordinator
        .program(chart_id)?
        .vars
        .iter()
        .find(|item| item.name == var)?;
    let object = objects
        .get(&boid)
        .filter(|object| object.identity == identity && object.fighter == fighter);
    Some(
        object
            .and_then(|object| object.state.chart(chart_id)?.values.get(var).copied())
            .unwrap_or(definition.initial),
    )
}

/// External events always select one exact chart. Legacy callers use `main`;
/// absent main never falls through to the compiled baseline.
pub fn dispatch_controller_named(
    boid: u32,
    fighter: &str,
    chart_id: &str,
    name: &str,
) -> Result<(), String> {
    let (coordinator, source) = active_for_object(boid, fighter)?;
    let program = coordinator
        .program(chart_id)
        .ok_or_else(|| format!("unknown controller chart {chart_id:?}"))?;
    let binding = program
        .event_bindings
        .iter()
        .find(|binding| binding.name == name)
        .ok_or_else(|| format!("chart {chart_id:?} has no named event {name:?}"))?;
    if !binding.fields.is_empty() {
        return Err(format!("chart {chart_id:?} event {name:?} requires typed arguments and cannot be used as a root event"));
    }
    let inputs = program.native_inputs.clone();
    let (context, mut state) = begin(boid, fighter, coordinator, source, None)?;
    let snapshot = super::super::native_values::sample_behavior_inputs(
        boid,
        fighter,
        context.identity,
        &inputs,
    )?;
    if !context.is_current() {
        return Err("controller dispatch context changed while sampling named event inputs".into());
    }
    let result = context
        .coordinator
        .step_named_event(&mut state, chart_id, name, &snapshot)?;
    accept_dispatch(&context, state, &result.engine_actions)
}

/// Dispatch constant typed values to one exact controller chart. Argument
/// validation runs against the selected immutable program before actor state
/// is reserved or native inputs are sampled.
pub fn dispatch_controller_named_values(
    boid: u32,
    fighter: &str,
    chart_id: &str,
    name: &str,
    arguments: &[(&str, model::Value)],
) -> Result<(), String> {
    let (coordinator, source) = active_for_object(boid, fighter)?;
    let program = coordinator
        .program(chart_id)
        .ok_or_else(|| format!("unknown controller chart {chart_id:?}"))?;
    program
        .validate_named_event_values(name, arguments)
        .map_err(|error| format!("chart {chart_id:?}: {error}"))?;
    let inputs = program.native_inputs.clone();
    let (context, mut state) = begin(boid, fighter, coordinator, source, None)?;
    let snapshot = super::super::native_values::sample_behavior_inputs(
        boid,
        fighter,
        context.identity,
        &inputs,
    )?;
    if !context.is_current() {
        return Err("controller dispatch context changed while sampling named event inputs".into());
    }
    let result = context
        .coordinator
        .step_named_event_values(&mut state, chart_id, name, arguments, &snapshot)?;
    accept_dispatch(&context, state, &result.engine_actions)
}

pub fn restart_controller_chart(
    fighter: &str,
    boid: u32,
    chart_id: &str,
    expected_revision: u64,
) -> Ack {
    let (coordinator, source) = match active(fighter) {
        Ok((_, DispatchSource::Compiled)) => {
            return Ack::rejected(
                expected_revision,
                "compiled baseline restart requires a separate accepted baseline identity",
            )
        }
        Ok((coordinator, source @ DispatchSource::Live { .. })) => (coordinator, source),
        Ok((_, DispatchSource::Status { .. })) => {
            return Ack::rejected(expected_revision, "cannot restart an active status owner")
        }
        Err(error) => return Ack::rejected(expected_revision, error),
    };
    let DispatchSource::Live { revision } = source else {
        unreachable!("compiled restart returned above")
    };
    if revision != expected_revision {
        return Ack::rejected(
            expected_revision,
            "controller revision changed before restart",
        );
    }
    if coordinator.program(chart_id).is_none() {
        return Ack::rejected(
            expected_revision,
            format!("unknown controller chart {chart_id:?}"),
        );
    }
    let (context, mut state) = match begin(boid, fighter, coordinator, source, None) {
        Ok(context) => context,
        Err(error) => return Ack::rejected(expected_revision, error),
    };
    if let Err(error) = context
        .coordinator
        .restart_chart(&mut state, chart_id)
        .and_then(|()| context.commit(state))
    {
        return Ack::rejected(expected_revision, error);
    }
    Ack::accepted(
        expected_revision,
        format!("restarted {fighter} object {boid} chart {chart_id:?}"),
    )
}

fn consumes(
    coordinator: &ControllerCoordinator,
    hook: model::HookName,
    status: Option<i32>,
    resource: Option<&model::ResourceSource>,
) -> bool {
    // Timers advance on frames even in a chart with no charge-tick binding.
    if hook == model::HookName::OnChargeTick || hook == model::HookName::OnStockStart {
        return true;
    }
    // Generic timer notifications have no chart-local causal expiry proof.
    if hook == model::HookName::OnTimeout && resource.is_none() {
        return false;
    }
    coordinator.chart_ids().any(|chart| {
        coordinator
            .program(chart)
            .is_some_and(|program| hook_available_for_program(program, hook, status, resource))
    })
}

pub(super) fn dispatch(
    hook: model::HookName,
    boid: u32,
    fighter: &str,
    status: Option<i32>,
    resource: Option<model::ResourceSource>,
    contact: Option<super::super::damage_dispatch::Event>,
) {
    if hook == model::HookName::OnChargeTick && RUNTIME.lock().pending_dispatch.contains_key(&boid)
    {
        return;
    }
    let Ok((coordinator, source)) = active_for_object(boid, fighter) else {
        return;
    };
    if !consumes(&coordinator, hook, status, resource.as_ref()) {
        return;
    }
    let outcome = (|| -> Result<(), String> {
        let inputs = coordinator.native_inputs();
        let (context, mut state) = begin(boid, fighter, coordinator, source, contact)?;
        let mut snapshot = super::super::native_values::sample_behavior_inputs_guarded(
            boid,
            fighter,
            context.identity,
            &inputs,
            || super::super::damage_context::require_current(contact, boid, context.identity),
        )?;
        if matches!(
            hook,
            model::HookName::OnChargeTick
                | model::HookName::OnSwitch
                | model::HookName::OnItemPickup
                | model::HookName::OnItemThrown
                | model::HookName::OnGrab
                | model::HookName::OnThrow
                | model::HookName::OnGrabbed
        ) && context.coordinator.chart_ids().any(|chart| {
            context
                .coordinator
                .program(chart)
                .is_some_and(model::LiveProgram::uses_phase_input)
        }) {
            snapshot.phase = Some(super::super::phase_control::sample(
                boid,
                fighter,
                context.identity,
            )?);
        }
        if matches!(
            hook,
            model::HookName::OnChargeTick | model::HookName::OnSwitch
        ) && context.coordinator.chart_ids().any(|chart| {
            context
                .coordinator
                .program(chart)
                .is_some_and(model::LiveProgram::uses_controller_input)
        }) {
            snapshot.controller = Some(super::super::native_values::sample_controller_input(
                boid,
                fighter,
                context.identity,
            )?);
        }
        if !context.is_current() {
            return Err("controller dispatch context changed while sampling inputs".into());
        }
        let result = if let Some(event) = contact {
            context.coordinator.step_damage(
                &mut state,
                super::super::damage_context::input(event)?,
                &snapshot,
            )
        } else if hook == model::HookName::OnChargeTick {
            context.coordinator.step_frame(&mut state, &snapshot)
        } else if let Some(source) = resource {
            context
                .coordinator
                .step_resource(&mut state, source, &snapshot)
        } else if let Some(status) = status {
            context
                .coordinator
                .step_status(&mut state, hook, status, &snapshot)
        } else {
            context.coordinator.step_hook(&mut state, hook, &snapshot)
        }?;
        let carried = state.clone();
        accept_dispatch(&context, state, &result.engine_actions)?;
        form_switch::capture_committed(&context, carried);
        Ok(())
    })();
    if let Err(error) = outcome {
        crate::slight::diag::note(format!("controllers {fighter} object {boid}: {error}"));
    }
}

/// Read-only rows include each chart's identity; no runtime slots are created
/// by observing a roster. V4 continues to use its unchanged `StateRow` shape.
#[cfg(test)]
pub fn controller_wire_rows() -> Vec<ControllerStateRow> {
    wire_snapshot().map(|(_, rows)| rows).unwrap_or_default()
}

pub(super) fn wire_snapshot() -> Option<(u64, Vec<ControllerStateRow>)> {
    let identities: Vec<_> = crate::slight::agents::all_records()
        .into_iter()
        .filter(|record| record.category == 0 && (0..8).contains(&record.entry_id))
        .filter_map(|record| {
            let fighter = registry::fighter_name(record.kind)?;
            let identity = Identity::from_live_fighter(record.boid, fighter)?;
            Some((record, fighter, identity))
        })
        .collect();
    let runtime = RUNTIME.lock();
    let has_effective_v5 = !runtime.controllers.is_empty()
        || runtime
            .compiled_controllers
            .keys()
            .any(|fighter| effective_compiled_owner(&runtime, fighter));
    if !has_effective_v5 {
        return None;
    }
    let mut rows = Vec::new();
    let mut accepted_actors = 0usize;
    for (record, fighter, identity) in identities {
        let live_v5 = runtime.controllers.get(fighter);
        let compiled_v5 = (!runtime.disabled_fighters.contains(fighter))
            .then(|| runtime.compiled_controllers.get(fighter))
            .flatten();
        if live_v5.is_none() && compiled_v5.is_none() {
            continue;
        }
        if accepted_actors >= MAX_LIVE_ACTORS {
            crate::slight::diag::note(
                "controller telemetry actor limit exceeded; snapshot omitted".to_owned(),
            );
            return Some((runtime.revision, Vec::new()));
        }
        accepted_actors += 1;
        if let Some(coordinator) = live_v5.or(compiled_v5) {
            let object = if live_v5.is_some() {
                runtime.controller_objects.get(&record.boid)
            } else {
                runtime.compiled_controller_objects.get(&record.boid)
            };
            let state = object
                .filter(|object| object.identity == identity && object.fighter == fighter)
                .map(|object| object.state.clone())
                .unwrap_or_else(|| coordinator.initial_state());
            for chart_id in coordinator.chart_ids() {
                let (Some(program), Some(chart)) =
                    (coordinator.program(chart_id), state.chart(chart_id))
                else {
                    continue;
                };
                rows.push(ControllerStateRow {
                    actor: StateRow {
                        fighter: fighter.into(),
                        boid: record.boid,
                        entry_id: record.entry_id,
                        revision: runtime.revision,
                        state: chart.state.clone(),
                        vars: chart.values.clone(),
                    },
                    chart_id: chart_id.into(),
                    program_id: program.id.clone(),
                    stock_generation: identity.stock_generation,
                });
            }
        }
    }
    rows.sort_by(|left, right| {
        (left.actor.entry_id, left.actor.boid, &left.chart_id).cmp(&(
            right.actor.entry_id,
            right.actor.boid,
            &right.chart_id,
        ))
    });
    if rows.len() > MAX_LIVE_ACTORS * super::super::controller_runtime::MAX_CHARTS {
        crate::slight::diag::note(
            "controller telemetry row limit exceeded; snapshot omitted".to_owned(),
        );
        return Some((runtime.revision, Vec::new()));
    }
    if serde_json::to_writer(&mut BoundedTelemetry(Vec::new()), &rows).is_err() {
        crate::slight::diag::note(
            "controller telemetry payload cap exceeded; snapshot omitted".to_owned(),
        );
        return Some((runtime.revision, Vec::new()));
    }
    Some((runtime.revision, rows))
}

pub(super) fn resource_consumer(runtime: &Runtime) -> bool {
    let consumes = |coordinator: &ControllerCoordinator| {
        coordinator.chart_ids().any(|chart| {
            coordinator
                .program(chart)
                .is_some_and(|program| !program.resource_triggers.is_empty())
        })
    };
    runtime
        .controllers
        .values()
        .any(|coordinator| consumes(coordinator))
        || runtime
            .compiled_controllers
            .iter()
            .any(|(fighter, coordinator)| {
                effective_compiled_owner(runtime, fighter) && consumes(coordinator)
            })
}

pub(super) struct StatusLease {
    pub(super) coordinator: Arc<ControllerCoordinator>,
    object: ObjectState,
    status: i32,
    epoch: u64,
    seen: BTreeSet<super::super::status_model::Phase>,
    last_tick: Option<i32>,
    initialized: bool,
}

fn active_for_object(
    boid: u32,
    fighter: &str,
) -> Result<(Arc<ControllerCoordinator>, DispatchSource), String> {
    if let Some(lease) = RUNTIME.lock().status_leases.get(&boid) {
        if lease.object.fighter == fighter {
            return Ok((
                Arc::clone(&lease.coordinator),
                DispatchSource::Status { epoch: lease.epoch },
            ));
        }
    }
    let (coordinator, source) = active(fighter)?;
    // A gimmick made for some costumes leaves the fighter's other costumes alone.
    let module = unsafe { smash::app::sv_battle_object::module_accessor(boid) };
    let costume = unsafe {
        smash::app::lua_bind::WorkModule::get_int(
            module,
            *smash::lib::lua_const::FIGHTER_INSTANCE_WORK_ID_INT_COLOR,
        )
    };
    if module.is_null()
        || !u8::try_from(costume).is_ok_and(|costume| coordinator.runs_for_costume(costume))
    {
        return Err("this costume has no gimmick".into());
    }
    Ok((coordinator, source))
}

pub(crate) fn owns_status(boid: u32, status: i32) -> bool {
    RUNTIME
        .lock()
        .status_leases
        .get(&boid)
        .is_some_and(|l| l.status == status && l.initialized)
}
/// Whether the chart holding this fighter's status lease also owns `status`.
pub(crate) fn owns_status_of_chart(boid: u32, status: i32) -> bool {
    use super::super::status_model::Policy;
    RUNTIME.lock().status_leases.get(&boid).is_some_and(|l| {
        l.coordinator
            .status_owner(status)
            .is_some_and(|(_, b)| b.binding.policy == Policy::Owned)
    })
}
pub(crate) fn abort_status_pre(boid: u32, status: i32) {
    let mut runtime = RUNTIME.lock();
    if runtime
        .status_leases
        .get(&boid)
        .is_some_and(|l| l.status == status && !l.initialized)
    {
        runtime.status_leases.remove(&boid);
    }
}

pub(crate) fn status_inputs(
    boid: u32,
    fighter: &str,
    status: i32,
    phase: super::super::status_model::Phase,
) -> Result<
    Option<(
        Identity,
        Vec<super::super::native_inputs::NativeInput>,
        bool,
    )>,
    String,
> {
    use super::super::status_model::Phase;
    let identity =
        Identity::from_live_fighter(boid, fighter).ok_or("status actor is unavailable")?;
    if phase == Phase::Pre {
        let (coordinator, source) = match active(fighter) {
            Ok(active) => active,
            Err(_) => return Ok(None),
        };
        if coordinator.status_owner(status).is_none() {
            return Ok(None);
        }
        let mut runtime = RUNTIME.lock();
        if runtime
            .pending_status_update
            .as_ref()
            .is_some_and(|(_, _, disabled)| disabled.contains(fighter))
        {
            return Ok(None);
        }
        if !actor_slot_available(&runtime, boid) {
            return Err(ACTOR_CAPACITY_ERROR.into());
        }
        let object = match source {
            DispatchSource::Live { .. } => runtime.controller_objects.remove(&boid),
            _ => runtime.compiled_controller_objects.remove(&boid),
        };
        let state = object
            .filter(|o| o.identity == identity)
            .map(|o| o.state)
            .unwrap_or_else(|| coordinator.initial_state());
        runtime.next_status_epoch = runtime.next_status_epoch.wrapping_add(1).max(1);
        let epoch = runtime.next_status_epoch;
        runtime.status_leases.insert(
            boid,
            StatusLease {
                coordinator,
                object: ObjectState {
                    identity,
                    fighter: fighter.into(),
                    state,
                },
                status,
                epoch,
                seen: BTreeSet::new(),
                last_tick: None,
                initialized: false,
            },
        );
        crate::slight::diag::note(format!(
            "status entry {fighter}/{status} epoch={epoch} revision={}",
            runtime.revision
        ));
    }
    let runtime = RUNTIME.lock();
    let Some(lease) = runtime
        .status_leases
        .get(&boid)
        .filter(|l| l.object.identity == identity && l.status == status)
    else {
        return Ok(None);
    };
    let uses_controller = lease.coordinator.chart_ids().any(|chart| {
        lease
            .coordinator
            .program(chart)
            .is_some_and(model::LiveProgram::uses_controller_input)
    });
    Ok(Some((
        identity,
        lease.coordinator.native_inputs(),
        uses_controller,
    )))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn status_phase(
    boid: u32,
    fighter: &str,
    status: i32,
    phase: super::super::status_model::Phase,
    snapshot: &super::super::native_inputs::NativeSnapshot,
    mut preflight: impl FnMut(&model::Action) -> Result<(), String>,
    mut deliver: impl FnMut(&model::Action) -> Result<bool, String>,
) -> Result<Option<bool>, String> {
    use super::super::{
        status_commands::Command,
        status_model::{Phase, Policy},
    };
    let (coordinator, source) = active_for_object(boid, fighter)?;
    let (epoch, owned) = {
        let mut runtime = RUNTIME.lock();
        let Some(lease) = runtime
            .status_leases
            .get_mut(&boid)
            .filter(|l| l.status == status)
        else {
            return Ok(None);
        };
        let owned = lease
            .coordinator
            .status_owner(status)
            .is_some_and(|(_, b)| b.binding.policy == Policy::Owned);
        if snapshot.stopped && matches!(phase, Phase::MainLoop | Phase::Exec) {
            return Ok(owned.then_some(false));
        }
        if matches!(phase, Phase::MainLoop | Phase::Exec) {
            if phase == Phase::MainLoop {
                let tick = snapshot
                    .phase
                    .as_ref()
                    .ok_or("status phase snapshot is missing")?
                    .status_frame;
                if lease.last_tick == Some(tick) {
                    return Ok(owned.then_some(false));
                }
                lease.last_tick = Some(tick);
            }
        } else if !lease.seen.insert(phase) {
            return Ok(owned.then_some(false));
        }
        (lease.epoch, owned)
    };
    let (context, mut state) = begin(boid, fighter, coordinator, source, None)?;
    let mut changed = false;
    let mut action_count = 0usize;
    for (chunk, roots) in context
        .coordinator
        .status_root_chunks(status, phase)?
        .iter()
        .enumerate()
    {
        let result = context.coordinator.step_status_roots(
            &mut state,
            status,
            phase,
            roots,
            chunk == 0,
            snapshot,
        )?;
        action_count += result.engine_actions.len();
        if action_count > super::super::controller_runtime::MAX_ENGINE_ACTIONS_PER_STEP {
            return Err("status action budget exceeded".into());
        }
        for tagged in &result.engine_actions {
            if matches!(tagged.action, model::Action::Status(_)) {
                preflight(&tagged.action)?;
            } else {
                preflight_actions(&context, std::slice::from_ref(tagged))?;
            }
        }
        context.commit(state.clone())?;
        for tagged in result.engine_actions {
            if !context.is_current() {
                return Err("status entry changed before delivery".into());
            }
            if let model::Action::Status(command) = &tagged.action {
                if deliver(&tagged.action)? {
                    changed = true;
                    break;
                }
                if matches!(command, Command::InitializeStatus(_)) {
                    if let Some(lease) = RUNTIME
                        .lock()
                        .status_leases
                        .get_mut(&boid)
                        .filter(|l| l.epoch == epoch)
                    {
                        lease.initialized = true;
                    }
                }
                if matches!(command, Command::ChangeStatusNow { .. }) {
                    changed = true;
                    break;
                }
            } else {
                let program = context
                    .coordinator
                    .program(&tagged.chart_id)
                    .ok_or("status chart is missing")?;
                execute_engine_action(program, boid, context.identity, &tagged.action)?;
                if matches!(tagged.action, model::Action::RequestFighterStatus { .. }) {
                    changed = true;
                    break;
                }
            }
        }
        if changed {
            break;
        }
    }
    if !matches!(phase, Phase::MainLoop | Phase::Exec) {
        crate::slight::diag::note(format!(
            "status phase {fighter}/{status} epoch={epoch} {phase:?}"
        ));
    }
    drop(context);
    if phase == Phase::Exit {
        finish_status(boid, epoch);
    }
    Ok(owned.then_some(changed))
}

fn finish_status(boid: u32, epoch: u64) {
    let (lease, pending) = {
        let mut runtime = RUNTIME.lock();
        if runtime
            .status_leases
            .get(&boid)
            .is_none_or(|l| l.epoch != epoch)
        {
            return;
        }
        let lease = runtime.status_leases.remove(&boid);
        let pending = if runtime.status_leases.is_empty() {
            runtime.pending_status_update.take()
        } else {
            None
        };
        (lease, pending)
    };
    if let Some(lease) = lease {
        if let Ok((next, source)) = active(&lease.object.fighter) {
            if let Ok(state) = next.reconcile_state(&lease.coordinator, &lease.object.state) {
                let object = ObjectState {
                    state,
                    ..lease.object
                };
                let mut runtime = RUNTIME.lock();
                match source {
                    DispatchSource::Live { .. } => {
                        runtime.controller_objects.insert(boid, object);
                    }
                    DispatchSource::Compiled => {
                        runtime.compiled_controller_objects.insert(boid, object);
                    }
                    _ => {}
                }
            }
        }
    }
    if let Some((_, raw, _)) = pending {
        let ack = replace(&raw);
        crate::slight::diag::note(format!("status revision after Exit: {}", ack.message));
    }
}

/// Explicit receiver contacts share guarded controller dispatch. Incoming-hit
/// cancellation finishes inside the veto callback; deflections arrive sealed.
pub(super) fn dispatch_incoming(
    boid: u32,
    input: super::super::native_inputs::DamageInput,
    current: impl Fn() -> bool,
) -> Result<bool, String> {
    let record = crate::slight::agents::lookup(boid).ok_or("contact receiver missing")?;
    let fighter = registry::fighter_name(record.kind).ok_or("contact receiver kind missing")?;
    let Ok((coordinator, source)) = active_for_object(boid, fighter) else {
        return Ok(false);
    };
    let hook = match input.direction() {
        super::super::native_inputs::DamageDirection::Incoming => model::HookName::OnIncomingHit,
        super::super::native_inputs::DamageDirection::Reflect => model::HookName::OnReflect,
        super::super::native_inputs::DamageDirection::Absorb => model::HookName::OnAbsorb,
        _ => return Err("unsupported receiver contact".into()),
    };
    if !consumes(&coordinator, hook, None, None) {
        return Ok(false);
    }
    if RUNTIME.lock().pending_dispatch.contains_key(&boid) || !current() {
        return Ok(false);
    }
    let inputs = coordinator.native_inputs();
    let (context, mut state) = begin(boid, fighter, coordinator, source, None)?;
    let check = || {
        if current() && context.is_current() {
            Ok(())
        } else {
            Err("receiver contact identity or configuration changed".to_string())
        }
    };
    let mut snapshot = super::super::native_values::sample_behavior_inputs_guarded(
        boid,
        fighter,
        context.identity,
        &inputs,
        check,
    )?;
    if context.coordinator.chart_ids().any(|chart| {
        context
            .coordinator
            .program(chart)
            .is_some_and(model::LiveProgram::uses_phase_input)
    }) {
        snapshot.phase = Some(super::super::phase_control::sample(
            boid,
            fighter,
            context.identity,
        )?);
    }
    if context.coordinator.chart_ids().any(|chart| {
        context
            .coordinator
            .program(chart)
            .is_some_and(model::LiveProgram::uses_controller_input)
    }) {
        snapshot.controller = Some(super::super::native_values::sample_controller_input(
            boid,
            fighter,
            context.identity,
        )?);
    }
    check()?;
    let mut result = context
        .coordinator
        .step_damage(&mut state, input, &snapshot)?;
    let cancel = result
        .engine_actions
        .iter()
        .any(|a| matches!(a.action, model::Action::CancelIncomingHit));
    result
        .engine_actions
        .retain(|a| !matches!(a.action, model::Action::CancelIncomingHit));
    preflight_actions(&context, &result.engine_actions)?;
    check()?;
    accept_dispatch(&context, state, &result.engine_actions)?;
    check()?;
    Ok(cancel)
}
