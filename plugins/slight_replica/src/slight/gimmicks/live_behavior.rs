// Owned live behavior snapshots layered over the compiled behavior baseline.
// This module is intentionally separate from the static interpreter. The
// desktop sends a bounded data-only `LiveProgram`; strings and vectors remain
// owned, and replacement drops the previous Arc instead of leaking a new
// static artifact on every edit.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;
use serde::Serialize;

use super::{dispatcher, live_model as model, registry};

#[path = "live_behavior/controllers.rs"]
mod controllers;
#[cfg(test)]
pub use controllers::{controller_wire_rows, ControllerStateRow};
pub use controllers::{
    dispatch_controller_named, dispatch_controller_named_values, read_controller_value,
    restart_controller_chart,
};

const MAX_REQUEST_BYTES: usize = model::MAX_SNAPSHOT_BYTES;
const MAX_LIVE_ACTORS: usize = 128;
const ACTOR_CAPACITY_ERROR: &str = "live behavior actor-state capacity (128 actors) is full";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ack {
    pub version: u8,
    pub family: &'static str,
    pub revision: u64,
    pub accepted: bool,
    pub message: String,
}

impl Ack {
    fn accepted(revision: u64, message: impl Into<String>) -> Self {
        Self {
            version: 1,
            family: "behavior",
            revision,
            accepted: true,
            message: message.into(),
        }
    }

    fn rejected(revision: u64, message: impl Into<String>) -> Self {
        Self {
            version: 1,
            family: "behavior",
            revision,
            accepted: false,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StateRow {
    pub fighter: String,
    pub boid: u32,
    pub entry_id: i32,
    pub revision: u64,
    pub state: String,
    pub vars: BTreeMap<String, model::TypedValue>,
}

pub(crate) use super::actor_identity::Identity;

/// Revalidate an actor against the engine's current fighter identity. This is
/// shared with generated static dispatch and object-action guards.
pub fn actor_matches(boid: u32, fighter: &str) -> bool {
    Identity::from_live_fighter(boid, fighter).is_some()
}

#[cfg(test)]
pub(crate) fn test_actor_slot_counts() -> (usize, usize, usize) {
    let runtime = RUNTIME.lock();
    let mut retained = BTreeSet::new();
    retained.extend(runtime.controller_objects.keys().copied());
    retained.extend(runtime.compiled_controller_objects.keys().copied());
    (
        retained.len(),
        runtime.pending_dispatch.len(),
        actor_slot_count(&runtime),
    )
}

#[derive(Default)]
struct Runtime {
    revision: u64,
    next_dispatch_ticket: u64,
    /// Shares the explicit actor-slot bound with retained live state. Changes
    /// to one BOID invalidate only that actor's in-flight dispatch.
    pending_dispatch: BTreeMap<u32, PendingDispatch>,
    has_snapshot: bool,
    status_leases: BTreeMap<u32, controllers::StatusLease>,
    next_status_epoch: u64,
    status_manifest: BTreeSet<(String, u32, u8)>,
    custom_statuses: BTreeMap<(String, String), u8>,
    pending_status_update: Option<(u64, String, BTreeSet<String>)>,
    disabled_fighters: BTreeSet<String>,
    controllers: BTreeMap<String, Arc<super::controller_runtime::ControllerCoordinator>>,
    controller_objects: BTreeMap<u32, controllers::ObjectState>,
    compiled_controllers: BTreeMap<String, Arc<super::controller_runtime::ControllerCoordinator>>,
    compiled_controller_objects: BTreeMap<u32, controllers::ObjectState>,
}

/// Count retained actor states and in-flight reservations as one BOID union.
/// A reservation for an actor that already has state must not consume a
/// second slot, while an actor without state reserves its slot immediately.
fn actor_slot_count(runtime: &Runtime) -> usize {
    let mut actors = BTreeSet::new();
    actors.extend(runtime.controller_objects.keys().copied());
    actors.extend(runtime.compiled_controller_objects.keys().copied());
    actors.extend(runtime.pending_dispatch.keys().copied());
    actors.extend(runtime.status_leases.keys().copied());
    actors.len()
}

fn actor_slot_available(runtime: &Runtime, boid: u32) -> bool {
    runtime.controller_objects.contains_key(&boid)
        || runtime.status_leases.contains_key(&boid)
        || runtime.compiled_controller_objects.contains_key(&boid)
        || runtime.pending_dispatch.contains_key(&boid)
        || actor_slot_count(runtime) < MAX_LIVE_ACTORS
}

struct PendingDispatch {
    ticket: u64,
    fighter: String,
}

pub(crate) struct ActorDispatchTicket {
    boid: u32,
    ticket: u64,
    fighter: String,
}

impl Drop for ActorDispatchTicket {
    fn drop(&mut self) {
        let mut runtime = RUNTIME.lock();
        if runtime
            .pending_dispatch
            .get(&self.boid)
            .is_some_and(|pending| pending.ticket == self.ticket)
        {
            runtime.pending_dispatch.remove(&self.boid);
        }
    }
}

static RUNTIME: LazyLock<Mutex<Runtime>> = LazyLock::new(|| Mutex::new(Runtime::default()));
static CLOUD_ACTIVE_ACTIONS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn refresh_cloud_resource_demand(runtime: &Runtime) {
    let uses = |program: &model::LiveProgram| {
        program
            .required_native_features()
            .contains(super::native_inputs::CLOUD_LIMIT_ACTIONS_FEATURE)
    };
    let coordinator_uses = |coordinator: &super::controller_runtime::ControllerCoordinator| {
        coordinator
            .chart_ids()
            .any(|id| coordinator.program(id).is_some_and(uses))
    };
    let needed = if let Some(coordinator) = runtime.controllers.get("cloud") {
        coordinator_uses(coordinator)
    } else if runtime.disabled_fighters.contains("cloud") {
        false
    } else {
        runtime
            .compiled_controllers
            .get("cloud")
            .is_some_and(|coordinator| coordinator_uses(coordinator))
    };
    CLOUD_ACTIVE_ACTIONS.store(needed, std::sync::atomic::Ordering::Release);
}

pub(crate) fn cloud_resource_demand() -> bool {
    CLOUD_ACTIVE_ACTIONS.load(std::sync::atomic::Ordering::Acquire)
}

/// Decode, validate, then atomically replace the complete live snapshot.
pub fn replace(raw: &str) -> Ack {
    controllers::replace(raw)
}

fn request_revision(raw: &str) -> u64 {
    if raw.len() > MAX_REQUEST_BYTES {
        return 0;
    }
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|root| root.get("gimmick_behavior").cloned())
        .and_then(|body| body.get("revision").and_then(serde_json::Value::as_u64))
        .unwrap_or(0)
}

/// Name used by the game-link server for incoming live behavior messages.
pub fn handle(raw: &str) -> Ack {
    let ack = replace(raw);
    if ack.accepted {
        register_damage_routes();
    }
    ack
}

/// Install the immutable V5 baseline emitted by the generated static adapter.
/// This is not a network snapshot and never changes `revision` or ACK state.
pub fn install_compiled_controllers(raw: &str) -> Result<(), String> {
    controllers::install_compiled_baseline(raw)?;
    register_damage_routes();
    Ok(())
}

#[cfg(test)]
pub(crate) fn test_install_compiled_baseline(raw: &str) -> Result<(), String> {
    install_compiled_controllers(raw)
}

#[cfg(test)]
pub(crate) fn test_effective_controller_source(fighter: &str) -> Option<&'static str> {
    controllers::test_effective_source(fighter)
}

#[cfg(test)]
pub(crate) fn test_insert_hidden_baseline_state(boid: u32, fighter: &str) -> bool {
    controllers::test_insert_hidden_baseline_state(boid, fighter)
}

/// Full empty snapshot restores the compiled baseline and clears all live
/// state. Root sends tombstones only for project-owned graph removals.
pub fn clear_overrides() {
    let mut runtime = RUNTIME.lock();
    if runtime.has_snapshot
        || !runtime.controllers.is_empty()
        || !runtime.disabled_fighters.is_empty()
    {
        super::damage_control::configuration_changed();
        super::damage_feed::configuration_changed();
    }
    let changed: BTreeSet<_> = runtime
        .controllers
        .keys()
        .chain(runtime.disabled_fighters.iter())
        .cloned()
        .collect();
    runtime.disabled_fighters.clear();
    runtime.controllers.clear();
    runtime.controller_objects.clear();
    runtime.pending_status_update = None;
    runtime.has_snapshot = false;
    runtime.revision = 0;
    runtime
        .pending_dispatch
        .retain(|_, pending| !changed.contains(pending.fighter.as_str()));
    runtime
        .compiled_controller_objects
        .retain(|_, object| !changed.contains(object.fighter.as_str()));
    refresh_cloud_resource_demand(&runtime);
}

/// Remove the package-installed baseline during explicit plugin uninstall.
/// Network empty snapshots must call `clear_overrides`, not this function.
pub fn clear_compiled_baseline() {
    let mut runtime = RUNTIME.lock();
    runtime.status_leases.clear();
    runtime.status_manifest.clear();
    runtime.custom_statuses.clear();
    if !runtime.compiled_controllers.is_empty() {
        super::damage_control::configuration_changed();
        super::damage_feed::configuration_changed();
    }
    let owners: BTreeSet<_> = runtime.compiled_controllers.keys().cloned().collect();
    runtime.compiled_controllers.clear();
    runtime.compiled_controller_objects.clear();
    runtime
        .pending_dispatch
        .retain(|_, pending| !owners.contains(&pending.fighter));
    refresh_cloud_resource_demand(&runtime);
}

fn validate_engine_symbols(program: &model::LiveProgram) -> Result<(), String> {
    for trigger in &program.status_triggers {
        registry::resolve(
            &program.fighter,
            &trigger.symbol,
            registry::IdentifierKind::FighterStatus,
        )
        .map_err(|error| format!("status trigger {}: {error}", trigger.hook.wire_name()))?;
    }
    for trigger in &program.resource_triggers {
        if trigger.source.fighter() != program.fighter {
            return Err(format!(
                "resource source belongs to {}, not {}",
                trigger.source.fighter(),
                program.fighter
            ));
        }
    }
    for transition in &program.transitions {
        for action in &transition.actions {
            match action {
                model::Action::Status(c) => super::status_commands::validate(c, &program.fighter)?,
                model::Action::GenerateArticle { symbol }
                | model::Action::RemoveArticle { symbol }
                | model::Action::RemoveNewestArticle { symbol } => {
                    super::operations::validate_behavior_article(&program.fighter, symbol)?;
                }
                model::Action::ArticleMovement { symbol, op } => {
                    super::operations::validate_behavior_article(&program.fighter, symbol)?;
                    op.validate_for_article()?;
                }
                model::Action::Object { symbol, .. } => {
                    super::operations::validate_behavior_article(&program.fighter, symbol)?
                }
                model::Action::RequestArticleStatus { symbol, status } => {
                    super::operations::validate_behavior_article_status(
                        &program.fighter,
                        symbol,
                        status,
                    )?
                }
                model::Action::RequestFighterStatus { symbol } => {
                    super::operations::validate_behavior_fighter_status(&program.fighter, symbol)?;
                }
                model::Action::RequestWeaponStatus { target, symbol } => {
                    super::operations::validate_behavior_weapon_status(
                        &program.fighter,
                        target.battle_object_id,
                        symbol,
                    )?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

pub fn revision() -> u64 {
    RUNTIME.lock().revision
}

/// An explicit project override suppresses the compiled baseline even when
/// its graph is empty. An empty full snapshot removes all such overrides.
pub fn overrides(fighter: &str) -> bool {
    let runtime = RUNTIME.lock();
    runtime.controllers.contains_key(fighter) || runtime.disabled_fighters.contains(fighter)
}

/// Static compiled handlers call this early guard before interpreting their
/// own program. It is exact-kind checked against the same live agent registry.
pub fn suppress_compiled(boid: u32, fighter: &str) -> bool {
    Identity::from_live_fighter(boid, fighter).is_some()
        && (overrides(fighter) || controllers::compiled_owner_installed(fighter))
}

/// Reserve this BOID for one in-flight compiled event. The reservation is
/// actor-local and is invalidated by lifecycle changes or a live override for
/// this fighter, but not by unrelated snapshot/state updates.
pub(crate) fn begin_compiled_dispatch(
    boid: u32,
    fighter: &str,
    identity: Identity,
) -> Option<ActorDispatchTicket> {
    if Identity::from_live_fighter(boid, fighter) != Some(identity) {
        return None;
    }
    let mut runtime = RUNTIME.lock();
    if runtime.controllers.contains_key(fighter)
        || runtime.disabled_fighters.contains(fighter)
        || runtime.compiled_controllers.contains_key(fighter)
    {
        return None;
    }
    if !actor_slot_available(&runtime, boid) {
        return None;
    }
    runtime.next_dispatch_ticket = runtime.next_dispatch_ticket.wrapping_add(1).max(1);
    let ticket = runtime.next_dispatch_ticket;
    runtime.pending_dispatch.insert(
        boid,
        PendingDispatch {
            ticket,
            fighter: fighter.to_owned(),
        },
    );
    Some(ActorDispatchTicket {
        boid,
        ticket,
        fighter: fighter.to_owned(),
    })
}

pub(crate) fn compiled_dispatch_is_current(
    identity: Identity,
    ticket: &ActorDispatchTicket,
    state: &super::state::Transaction,
) -> bool {
    if Identity::from_live_fighter(ticket.boid, &ticket.fighter) != Some(identity)
        || !state.is_current()
    {
        return false;
    }
    let runtime = RUNTIME.lock();
    runtime
        .pending_dispatch
        .get(&ticket.boid)
        .is_some_and(|pending| {
            pending.ticket == ticket.ticket
                && pending.fighter == ticket.fighter
                && !runtime.controllers.contains_key(&ticket.fighter)
                && !runtime.disabled_fighters.contains(&ticket.fighter)
                && !runtime.compiled_controllers.contains_key(&ticket.fighter)
        })
}

/// Identity/program-ticket check usable immediately after sampling and before
/// opening the authored-state transaction.
pub(crate) fn compiled_dispatch_ticket_is_current(
    identity: Identity,
    ticket: &ActorDispatchTicket,
) -> bool {
    if Identity::from_live_fighter(ticket.boid, &ticket.fighter) != Some(identity) {
        return false;
    }
    let runtime = RUNTIME.lock();
    runtime
        .pending_dispatch
        .get(&ticket.boid)
        .is_some_and(|pending| {
            pending.ticket == ticket.ticket
                && pending.fighter == ticket.fighter
                && !runtime.controllers.contains_key(&ticket.fighter)
                && !runtime.disabled_fighters.contains(&ticket.fighter)
                && !runtime.compiled_controllers.contains_key(&ticket.fighter)
        })
}

pub(crate) fn commit_compiled_dispatch(
    identity: Identity,
    ticket: &ActorDispatchTicket,
    state: super::state::Transaction,
    writes: &BTreeMap<String, super::state::VarValue>,
    contact: Option<super::damage_dispatch::Event>,
) -> bool {
    if Identity::from_live_fighter(ticket.boid, &ticket.fighter) != Some(identity)
        || !super::damage_context::is_current(contact, ticket.boid, identity)
    {
        return false;
    }
    let runtime = RUNTIME.lock();
    let current = runtime
        .pending_dispatch
        .get(&ticket.boid)
        .is_some_and(|pending| {
            pending.ticket == ticket.ticket
                && pending.fighter == ticket.fighter
                && !runtime.controllers.contains_key(&ticket.fighter)
                && !runtime.disabled_fighters.contains(&ticket.fighter)
                && !runtime.compiled_controllers.contains_key(&ticket.fighter)
        });
    current && state.commit(writes)
}

/// Route a named event to an active live override when this actor's fighter
/// owns one. `None` means there is no relevant live override and permits the
/// caller to try the compiled program; once an override is present, identity,
/// event, and evaluation failures are returned as `Some(Err)` so callers
/// never fall through to stale compiled behavior.
pub fn dispatch_named(boid: u32, fighter: &str, name: &str) -> Option<Result<(), String>> {
    let live = RUNTIME.lock().controllers.contains_key(fighter);
    if live || controllers::compiled_owner_effective(fighter) {
        return Some(dispatch_controller_named(boid, fighter, "main", name));
    }
    RUNTIME
        .lock()
        .disabled_fighters
        .contains(fighter)
        .then(|| Err(format!("live behavior for {fighter:?} is disabled")))
}

pub fn dispatch(hook: dispatcher::GimmickHook, boid: u32) {
    if matches!(hook, dispatcher::GimmickHook::Ko) {
        if let Some(record) = crate::slight::agents::lookup(boid) {
            controllers::clear_form_entry(record.entry_id);
        }
    }
    let (hook, resource) = match hook {
        dispatcher::GimmickHook::ItemPickup => (Some(model::HookName::OnItemPickup), None),
        dispatcher::GimmickHook::ItemThrown => (Some(model::HookName::OnItemThrown), None),
        dispatcher::GimmickHook::Grab => (Some(model::HookName::OnGrab), None),
        dispatcher::GimmickHook::Throw => (Some(model::HookName::OnThrow), None),
        dispatcher::GimmickHook::Grabbed => (Some(model::HookName::OnGrabbed), None),
        dispatcher::GimmickHook::ChargeTick => (Some(model::HookName::OnChargeTick), None),
        dispatcher::GimmickHook::Timeout => (Some(model::HookName::OnTimeout), None),
        dispatcher::GimmickHook::Spend => (None, None), // must arrive via dispatch_status
        dispatcher::GimmickHook::StatusEnter => (None, None), // exact status only
        dispatcher::GimmickHook::StatusExit => (None, None), // exact previous status only
        dispatcher::GimmickHook::Switch => (Some(model::HookName::OnSwitch), None),
        dispatcher::GimmickHook::StockStart => (Some(model::HookName::OnStockStart), None),
        dispatcher::GimmickHook::Ko => (Some(model::HookName::OnKo), None),
        dispatcher::GimmickHook::ArticleRemoved => (Some(model::HookName::OnArticleRemoved), None),
        dispatcher::GimmickHook::ArticleSpawn => (Some(model::HookName::OnArticleSpawn), None),
        dispatcher::GimmickHook::CloudLimitGroundedNeutralSpend => (
            Some(model::HookName::OnSpend),
            Some(model::ResourceSource::CloudLimitGroundedNeutral),
        ),
        dispatcher::GimmickHook::CloudLimitIdleExpiry => (
            Some(model::HookName::OnTimeout),
            Some(model::ResourceSource::CloudLimitIdleExpiry),
        ),
        _ => (None, None),
    };
    let Some(hook) = hook else {
        return;
    };
    dispatch_inner(hook, boid, None, resource.as_ref(), None);
}

/// Status-bound Spend is routed only after the exact status symbol is
/// resolved in the program fighter's namespace.
pub fn dispatch_status(status: i32, boid: u32) {
    dispatch_inner(model::HookName::OnSpend, boid, Some(status), None, None);
}

/// A native identity-aware status edge. Each authored status registration
/// selects only its own transition list; generic OnStatusEnter dispatch is
/// never enabled.
pub fn dispatch_status_enter(status: i32, boid: u32) {
    dispatch_inner(
        model::HookName::OnStatusEnter,
        boid,
        Some(status),
        None,
        None,
    );
}

/// A native identity-aware status edge. Exit registrations match the exact
/// previous status carried by the edge, independently of current-status entry.
pub fn dispatch_status_exit(status: i32, boid: u32) {
    dispatch_inner(
        model::HookName::OnStatusExit,
        boid,
        Some(status),
        None,
        None,
    );
}

/// Called outside the native capture hooks, before sealed contact delivery.
/// Reconnects deduplicate these typed functions for live and compiled charts.
pub(super) fn register_damage_routes() {
    let uses_damage = |program: &model::LiveProgram| {
        program.bindings.iter().any(|binding| {
            matches!(
                binding.hook,
                model::HookName::OnDealDamage
                    | model::HookName::OnTakeDamage
                    | model::HookName::OnAttackShielded
                    | model::HookName::OnShieldHit
                    | model::HookName::OnIncomingHit
                    | model::HookName::OnReflect
                    | model::HookName::OnAbsorb
            )
        })
    };
    let coordinator_uses_damage =
        |coordinator: &super::controller_runtime::ControllerCoordinator| {
            coordinator
                .chart_ids()
                .any(|chart| coordinator.program(chart).is_some_and(uses_damage))
        };
    let runtime = RUNTIME.lock();
    let needed = runtime
        .controllers
        .values()
        .any(|coordinator| coordinator_uses_damage(coordinator))
        || runtime
            .compiled_controllers
            .iter()
            .any(|(fighter, coordinator)| {
                !runtime.controllers.contains_key(fighter)
                    && !runtime.disabled_fighters.contains(fighter)
                    && coordinator_uses_damage(coordinator)
            });
    let incoming = runtime
        .controllers
        .values()
        .chain(
            runtime
                .compiled_controllers
                .iter()
                .filter(|(fighter, _)| {
                    !runtime.controllers.contains_key(*fighter)
                        && !runtime.disabled_fighters.contains(*fighter)
                })
                .map(|(_, coordinator)| coordinator),
        )
        .chain(
            runtime
                .status_leases
                .values()
                .map(|lease| &lease.coordinator),
        )
        .any(|coordinator| {
            coordinator.chart_ids().any(|chart| {
                coordinator.program(chart).is_some_and(|p| {
                    p.bindings
                        .iter()
                        .any(|b| b.hook == model::HookName::OnIncomingHit)
                })
            })
        });
    super::incoming_hit::set_enabled(incoming);
    drop(runtime);
    if needed {
        super::damage_dispatch::register(super::damage_dispatch::Direction::Deal, dispatch_damage);
        super::damage_dispatch::register(super::damage_dispatch::Direction::Take, dispatch_damage);
        super::damage_dispatch::register(
            super::damage_dispatch::Direction::AttackShielded,
            dispatch_damage,
        );
        super::damage_dispatch::register(
            super::damage_dispatch::Direction::ShieldHit,
            dispatch_damage,
        );
    }
}

/// Typed contact entry, independent of legacy object-only hook firings.
pub fn dispatch_damage(event: super::damage_dispatch::Event) {
    if !super::damage_feed::event_is_current(event) {
        return;
    }
    let hook = match event.direction() {
        super::damage_dispatch::Direction::Deal => model::HookName::OnDealDamage,
        super::damage_dispatch::Direction::Take => model::HookName::OnTakeDamage,
        super::damage_dispatch::Direction::AttackShielded => model::HookName::OnAttackShielded,
        super::damage_dispatch::Direction::ShieldHit => model::HookName::OnShieldHit,
    };
    dispatch_inner(
        hook,
        event.receiver().identity.object_id,
        None,
        None,
        Some(event),
    );
}

/// Observe handoffs only on the exact fighter's Main callback. The global
/// charge tick also visits inactive forms, before native activation is complete.
pub(crate) fn observe_form(boid: u32) {
    let Some(record) = crate::slight::agents::lookup(boid) else {
        return;
    };
    let Some(fighter) = registry::fighter_name(record.kind) else {
        return;
    };
    controllers::observe_form(boid, fighter);
}

fn dispatch_inner(
    hook: model::HookName,
    boid: u32,
    status: Option<i32>,
    resource: Option<&model::ResourceSource>,
    contact: Option<super::damage_dispatch::Event>,
) {
    let Some(mut record) = crate::slight::agents::lookup(boid) else {
        return;
    };
    // Nana is her own fighter object with no gimmicks of her own. Whatever happens to her in a
    // fight counts for the Ice Climbers' rules, which belong to Popo on the same player entry.
    let mut boid = boid;
    if contact.is_some() && record.kind == *smash::lib::lua_const::FIGHTER_KIND_NANA {
        let entry = record.entry_id;
        let Some(popo) = crate::slight::agents::all_records()
            .into_iter()
            .find(|other| {
                other.category == 0
                    && other.entry_id == entry
                    && other.kind == *smash::lib::lua_const::FIGHTER_KIND_POPO
            })
        else {
            return;
        };
        boid = popo.boid;
        record = popo;
    }
    let Some(fighter) = registry::fighter_name(record.kind) else {
        return;
    };
    let Some(identity) = Identity::from_live_fighter(boid, fighter) else {
        return;
    };
    if !super::damage_context::is_current(contact, boid, identity) {
        return;
    }
    if RUNTIME.lock().controllers.contains_key(fighter) {
        controllers::dispatch(hook, boid, fighter, status, resource.cloned(), contact);
        return;
    }
    if controllers::compiled_owner_effective(fighter) {
        controllers::dispatch(hook, boid, fighter, status, resource.cloned(), contact);
    }
}

fn hook_available_for_program(
    program: &model::LiveProgram,
    hook: model::HookName,
    status: Option<i32>,
    resource: Option<&model::ResourceSource>,
) -> bool {
    if !program.bindings.iter().any(|binding| binding.hook == hook) {
        return false;
    }
    if let Some(status) = status {
        return program.status_triggers.iter().any(|trigger| {
            trigger.hook == hook && {
                registry::resolve(
                    &program.fighter,
                    &trigger.symbol,
                    registry::IdentifierKind::FighterStatus,
                )
                .is_ok_and(|entry| i32::try_from(entry.value).ok() == Some(status))
            }
        });
    }
    if let Some(resource) = resource {
        return program
            .resource_triggers
            .iter()
            .any(|trigger| trigger.hook == hook && &trigger.source == resource);
    }
    match hook {
        model::HookName::OnSpend
        | model::HookName::OnStatusEnter
        | model::HookName::OnStatusExit => false,
        model::HookName::OnTimeout => !program
            .resource_triggers
            .iter()
            .any(|trigger| trigger.hook == model::HookName::OnTimeout),
        _ => true,
    }
}

fn preflight_engine_actions(
    boid: u32,
    identity: Identity,
    actions: &[model::Action],
) -> Result<(), String> {
    let grab_count = actions
        .iter()
        .filter(|a| {
            matches!(
                a,
                model::Action::AddHeldOpponentDamage { .. }
                    | model::Action::ThrowHeldOpponent { .. }
                    | model::Action::ReleaseHeldOpponent
            )
        })
        .count();
    if grab_count != 0 {
        let fighter =
            super::registry::fighter_name(identity.fighter_kind).ok_or("unknown grabber")?;
        super::grab_control::preflight(boid, fighter, identity, grab_count)?;
    }
    let damage_count = actions
        .iter()
        .filter(|action| {
            super::phase_commands::resolve(action).is_ok_and(super::damage_commands::is_damage)
        })
        .count();
    if damage_count != 0 {
        let fighter = super::registry::fighter_name(identity.fighter_kind)
            .ok_or("unknown fighter identity")?;
        super::damage_control::preflight(boid, fighter, identity, damage_count)?;
    }
    for action in actions {
        if matches!(
            action,
            model::Action::AddHeldOpponentDamage { .. }
                | model::Action::ThrowHeldOpponent { .. }
                | model::Action::ReleaseHeldOpponent
                | model::Action::AddDamage { .. }
                | model::Action::CombatPolicy { .. }
                | model::Action::Movement(_)
                | model::Action::Item(_)
                | model::Action::MeshVisibility { .. }
                | model::Action::SetMotionRate { .. }
                | model::Action::AddSpeed { .. }
        ) {
            let fighter = super::registry::fighter_name(identity.fighter_kind)
                .ok_or("unknown fighter identity")?;
            super::phase_control::preflight(boid, fighter, identity, action)?;
        }
        if let model::Action::SetNative { input, .. }
        | model::Action::AddNative { input, .. }
        | model::Action::SetNativeFlag { input, .. } = action
        {
            super::native_values::preflight_write(
                boid,
                input.fighter().unwrap_or(""),
                identity,
                action,
            )?;
        }
        if let model::Action::Resource(command) = action {
            super::resource_control::preflight(
                boid,
                identity,
                super::resource_control::resolved(command)?,
            )?;
        }
        if let model::Action::ChooseHeroMenuSpell { slot } = action {
            let fighter = super::registry::fighter_name(identity.fighter_kind)
                .ok_or("unknown fighter identity")?;
            super::hero_menu_control::preflight(boid, fighter, identity, *slot)?;
        }
    }
    Ok(())
}

fn execute_engine_action(
    program: &model::LiveProgram,
    boid: u32,
    identity: Identity,
    action: &model::Action,
) -> Result<(), String> {
    match action {
        model::Action::AddHeldOpponentDamage { .. }
        | model::Action::ThrowHeldOpponent { .. }
        | model::Action::ReleaseHeldOpponent
        | model::Action::AddDamage { .. }
        | model::Action::CombatPolicy { .. }
        | model::Action::Movement(_)
        | model::Action::Item(_)
        | model::Action::MeshVisibility { .. }
        | model::Action::SetMotionRate { .. }
        | model::Action::AddSpeed { .. } => {
            super::phase_control::execute(boid, &program.fighter, identity, action)
        }
        model::Action::SetNative { .. }
        | model::Action::AddNative { .. }
        | model::Action::SetNativeFlag { .. } => {
            super::native_values::execute_write(boid, &program.fighter, identity, action)
        }
        model::Action::Resource(command) => super::resource_control::execute(
            boid,
            identity,
            super::resource_control::resolved(command)?,
        ),
        model::Action::ChooseHeroMenuSpell { slot } => {
            super::hero_menu_control::execute(boid, &program.fighter, identity, *slot)
        }
        model::Action::GenerateArticle { symbol } => {
            super::operations::generate_article_from_behavior(&program.fighter, boid, symbol)
        }
        model::Action::RemoveNewestArticle { symbol } => {
            super::operations::remove_newest_article_from_behavior(&program.fighter, boid, symbol)
        }
        model::Action::ArticleMovement { symbol, op } => {
            super::operations::move_articles_from_behavior(&program.fighter, boid, symbol, *op)
        }
        model::Action::Object { symbol, op } => {
            let speed = match op {
                super::native_inputs::ObjectOp::SetSpeed {
                    x: model::Expr::Const(model::Value::Float(x)),
                    y: model::Expr::Const(model::Value::Float(y)),
                } => Some((*x, *y)),
                super::native_inputs::ObjectOp::SetSpeed { .. } => {
                    return Err("object speed was not resolved at its action".into())
                }
                super::native_inputs::ObjectOp::Remove => None,
            };
            super::deflect_volume::object_op(boid, &program.fighter, symbol, speed)
        }
        model::Action::RequestArticleStatus { symbol, status } => {
            super::operations::request_article_status_from_behavior(
                &program.fighter,
                boid,
                symbol,
                status,
            )
        }
        model::Action::RemoveArticle { symbol } => {
            super::operations::remove_article_from_behavior(&program.fighter, boid, symbol)
        }
        model::Action::Status(super::status_commands::Command::RequestStatus { target }) => {
            target.in_manifest(&program.custom_statuses)?;
            super::operations::request_resolved_status(
                &program.fighter,
                boid,
                target.resolve(&program.fighter)?,
            )
        }
        model::Action::RequestFighterStatus { symbol } => {
            super::operations::request_fighter_status_from_behavior(&program.fighter, boid, symbol)
        }
        model::Action::RequestWeaponStatus { target, symbol } => {
            super::operations::request_weapon_status_from_behavior(
                &program.fighter,
                boid,
                target.battle_object_id,
                symbol,
            )
        }
        _ => Ok(()),
    }
}

/// Reset only the state associated with one object. A reused BOID receives
/// fresh state automatically when the next event proves a different identity.
pub fn clear_stock(boid: u32) {
    if let Some(record) = crate::slight::agents::lookup(boid) {
        controllers::clear_form_entry(record.entry_id);
    }
    clear_boid(boid);
}

pub fn clear_boid(boid: u32) {
    let mut runtime = RUNTIME.lock();
    runtime.status_leases.remove(&boid);
    runtime.controller_objects.remove(&boid);
    runtime.compiled_controller_objects.remove(&boid);
    runtime.pending_dispatch.remove(&boid);
}

/// Match teardown clears per-object values while keeping live and compiled
/// controller configurations armed.
pub fn clear_all() {
    controllers::clear_forms();
    let mut runtime = RUNTIME.lock();
    runtime.status_leases.clear();
    runtime.pending_status_update = None;
    runtime.controller_objects.clear();
    runtime.compiled_controller_objects.clear();
    runtime.pending_dispatch.clear();
}

/// Restart one exact fighter object at its program entry state.
pub fn restart(fighter: &str, boid: u32, expected_revision: u64) -> Ack {
    restart_controller_chart(fighter, boid, "main", expected_revision)
}

/// Read-only current per-object values. Uninitialized objects show their
/// declared initial state without creating runtime state.
#[cfg(test)]
pub fn wire_rows() -> Vec<StateRow> {
    controllers::controller_wire_rows()
        .into_iter()
        .filter(|row| row.chart_id == "main")
        .map(|row| row.actor)
        .collect()
}

/// One gimmick value on this fighter, as a number, for an If block in a move.
pub fn chart_value(boid: u32, chart: &str, var: &str) -> Option<f32> {
    let (_, rows) = controllers::wire_snapshot()?;
    let row = rows
        .into_iter()
        .find(|row| row.actor.boid == boid && row.chart_id == chart)?;
    row.actor.vars.get(var).map(|value| match *value {
        model::TypedValue::Float(value) => value,
        model::TypedValue::Int(value) => value as f32,
        model::TypedValue::Flag(value) => f32::from(u8::from(value)),
    })
}

/// Capture the revision with its rows under the same configuration lock.
pub fn wire_snapshot() -> (u64, Vec<serde_json::Value>) {
    let (revision, rows) = controllers::wire_snapshot().unwrap_or_else(|| (revision(), Vec::new()));
    (
        revision,
        rows.into_iter().map(|row| serde_json::json!(row)).collect(),
    )
}

pub fn controller_capabilities(seq: u64) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "seq": seq,
        "request_version": super::controller_protocol::REQUEST_VERSION,
        "model_version": model::MODEL_VERSION,
        "max_charts": super::controller_runtime::MAX_CHARTS,
        "max_actors": MAX_LIVE_ACTORS,
        "max_rows": MAX_LIVE_ACTORS * super::controller_runtime::MAX_CHARTS,
    })
}

pub fn native_feature_capabilities(seq: u64) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "seq": seq,
        "features": [
            super::status_model::FEATURE,
            super::status_model::CANCEL_FEATURE,
            super::native_inputs::HELD_OPPONENT_FEATURE,
            super::native_inputs::GRAB_EVENTS_FEATURE,
            super::native_inputs::ITEM_EVENTS_FEATURE,
            super::native_inputs::COMMAND_INPUT_FEATURE,
            super::native_inputs::HELD_ITEM_KIND_FEATURE,
            super::native_inputs::INCOMING_HIT_FEATURE,
            super::native_inputs::REFLECT_ABSORB_FEATURE,
            super::native_inputs::MOVEMENT_FEATURE,
            super::native_inputs::ITEMS_FEATURE,
            super::resource_commands::FEATURE,
            super::native_inputs::FIGHTER_DAMAGE_FEATURE,
            super::native_inputs::PARTNER_FEATURE,
            super::native_inputs::OPPONENT_FEATURE,
            super::native_inputs::FIGHTER_STOCKS_FEATURE,
            super::native_inputs::FIGHTER_STATUS_IS_FEATURE,
            super::native_inputs::FIGHTER_STATUS_CHANGING_FEATURE,
            super::native_inputs::DAMAGE_CAPTURE_STATUS_FEATURE,
            super::native_inputs::CLOUD_LIMIT_ACTIONS_FEATURE,
            super::native_inputs::ARTICLE_PRESENCE_FEATURE,
            super::native_inputs::OWNED_ARTICLES_FEATURE,
            super::native_inputs::STATUS_PHASE_FEATURE,
            super::native_inputs::ATTACK_RESULT_FLAGS_FEATURE,
        ],
    })
}

/// Keeps the native resource observer armed while any effective owner
/// consumes its exact built-in resource feed.
pub fn resource_consumer() -> bool {
    controllers::resource_consumer(&RUNTIME.lock())
}

#[allow(unused_imports)]
pub(crate) use controllers::{
    abort_status_pre, owns_status, owns_status_of_chart, status_inputs, status_phase,
};
/// Explicit receiver contact, shared by live and packaged controllers.
pub(crate) fn dispatch_incoming(
    boid: u32,
    input: super::native_inputs::DamageInput,
    current: impl Fn() -> bool,
) -> bool {
    controllers::dispatch_incoming(boid, input, current).unwrap_or_else(|error| {
        crate::slight::diag::note(format!("receiver contact rejected: {error}"));
        false
    })
}
