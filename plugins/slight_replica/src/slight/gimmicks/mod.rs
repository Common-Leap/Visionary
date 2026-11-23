//! Gimmick runtime: dispatcher plus lifecycle (slice F2.4).
//!
//! Generated F2.3 behavior needs two runtime facts: hook firings delivered
//! to handlers, and variable state that dies with its object. This module
//! owns both, behind the same install-once lifecycle the other systems use:
//! `install` is idempotent so a reconnect that re-runs it cannot duplicate
//! handlers, and every teardown path wipes the state it owns.
//!
//! Wiring (in `systems::main_module`, one call per lifecycle point):
//!
//! * fighter init (spawn) → [`on_init_fighter`]: fresh stock state.
//! * exact identity-aware REBIRTH observation → fresh state plus the
//!   `StockStart` firing; legacy reinit notifications cannot duplicate it.
//! * fighter reset (KO, match exit) → [`on_reset_fighter`]: state wiped.
//! * weapon reset (despawn) → [`on_reset_weapon`]: article state wiped.
//! * uninstall → [`on_uninstall`]: handlers, state, and the install latch.
//!
//! Pure logic throughout — no engine calls — so the registry and the store
//! unit-test without the game. [`dispatcher::pump`] is the one frame feed;
//! generated per-hook functions call [`dispatcher::fire`] through it.

mod acmd_generated;
pub(crate) mod actor_identity;
pub(crate) mod article_context;
pub mod behavior;
mod chart_event_bridge;
mod cloud_resource;
mod cloud_resource_model;
mod contact_snapshot_model;
#[path = "../../../../../src/gimmicks/controller_protocol.rs"]
pub mod controller_protocol;
#[path = "../../../../../src/gimmicks/controller_runtime.rs"]
pub mod controller_runtime;
mod damage_capture;
mod damage_capture_model;
mod damage_commit;
mod damage_commit_model;
mod damage_context;
mod damage_delivery_model;
pub mod damage_dispatch;
mod damage_feed;
mod damage_feed_model;
mod damage_identity_model;
pub(crate) mod deflect_volume;
pub mod dispatcher;
mod fighter_status_queue;
mod generated;
#[path = "../../../../../src/gimmicks/grab_commands.rs"]
mod grab_commands;
pub(crate) mod grab_control;
#[path = "../../../../../src/gimmicks/hero_menu_commands.rs"]
pub mod hero_menu_commands;
mod hero_menu_control;
mod incoming_hit;
pub(crate) mod item_control;
pub mod live_behavior;
#[path = "../../../../../src/gimmicks/live_model.rs"]
pub mod live_model;
pub mod live_parameters;
#[path = "../../../../../src/gimmicks/live_params.rs"]
pub mod live_params;
pub mod meter_snapshot;
#[path = "../../../../../src/gimmicks/native_inputs.rs"]
pub mod native_inputs;
mod native_values;
pub mod operations;
mod partner_control;
#[path = "../../../../../src/gimmicks/phase_commands.rs"]
mod phase_commands;
mod phase_control;
mod receiver_deflect;
mod receiver_deflect_model;
pub mod registry;
#[path = "../../../../../src/gimmicks/resource_commands.rs"]
pub mod resource_commands;
mod resource_control;
mod shield_contacts;
mod shield_contacts_model;
pub mod state;

/// Closed scalar values accepted by the exact-chart ACMD event bridge.
pub use live_model::Value as ChartEventValue;

use std::sync::atomic::{AtomicBool, Ordering};

static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Source-export bridge from one native ACMD execution to its actor-owned
/// named event. Module identity is rechecked before selecting a program.
pub fn dispatch_named_from_module(module: *mut smash::app::BattleObjectModuleAccessor, name: &str) {
    let result = crate::slight::agents::boid_from_module(module)
        .ok_or_else(|| "ACMD named event has no live actor identity".to_owned())
        .and_then(|boid| generated::dispatch_named(boid, name).map(|()| boid));
    match result {
        Err(error) => {
            crate::slight::diag::note(format!("ACMD named event rejected: {error}"));
        }
        Ok(_) => {}
    }
}

/// Deliver a timed move event to one additional chart on this exact actor.
/// A live override that lacks the target cannot fall through to its baseline.
pub fn dispatch_named_chart_from_module(
    module: *mut smash::app::BattleObjectModuleAccessor,
    chart_id: &str,
    name: &str,
) {
    let result = crate::slight::agents::boid_from_module(module)
        .ok_or_else(|| "ACMD chart event has no live actor identity".to_owned())
        .and_then(|boid| chart_event_bridge::dispatch(boid, chart_id, name).map(|()| boid));
    match result {
        Err(error) => crate::slight::diag::note(format!("ACMD chart event rejected: {error}")),
        Ok(_) => {}
    }
}

/// Deliver a timed move event and bounded literal scalar values to one
/// additional chart on this exact actor. The event bridge never broadcasts.
pub fn dispatch_named_chart_values_from_module(
    module: *mut smash::app::BattleObjectModuleAccessor,
    chart_id: &str,
    name: &str,
    arguments: &[(&str, ChartEventValue)],
) {
    let result = crate::slight::agents::boid_from_module(module)
        .ok_or_else(|| "ACMD chart event has no live actor identity".to_owned())
        .and_then(|boid| {
            chart_event_bridge::dispatch_values(boid, chart_id, name, arguments).map(|()| boid)
        });
    match result {
        Err(error) => {
            crate::slight::diag::note(format!("ACMD chart event with values rejected: {error}"));
        }
        Ok(_) => {}
    }
}

fn controller_value_from_module(
    module: *mut smash::app::BattleObjectModuleAccessor,
    chart_id: &str,
    var: &str,
) -> Option<live_model::Value> {
    let boid = crate::slight::agents::boid_from_module(module)?;
    let record = crate::slight::agents::lookup(boid).filter(|record| record.category == 0)?;
    let fighter = registry::fighter_name(record.kind)?;
    if let Some(value) = live_behavior::read_controller_value(boid, fighter, chart_id, var) {
        return Some(value);
    }
    if !live_behavior::actor_matches(boid, fighter)
        || live_behavior::suppress_compiled(boid, fighter)
    {
        return None;
    }
    generated::read_controller_value(boid, chart_id, var)
}

/// Read an actor's effective Int or Timer variable; missing values return zero.
pub fn read_controller_int_from_module(
    module: *mut smash::app::BattleObjectModuleAccessor,
    chart_id: &str,
    var: &str,
) -> i32 {
    match controller_value_from_module(module, chart_id, var) {
        Some(live_model::Value::Int(value)) => value,
        _ => 0,
    }
}

/// Read an actor's effective Float variable; missing values return zero.
pub fn read_controller_float_from_module(
    module: *mut smash::app::BattleObjectModuleAccessor,
    chart_id: &str,
    var: &str,
) -> f32 {
    match controller_value_from_module(module, chart_id, var) {
        Some(live_model::Value::Float(value)) => value,
        _ => 0.0,
    }
}

/// Read an actor's effective Flag variable; missing values return false.
pub fn read_controller_flag_from_module(
    module: *mut smash::app::BattleObjectModuleAccessor,
    chart_id: &str,
    var: &str,
) -> bool {
    match controller_value_from_module(module, chart_id, var) {
        Some(live_model::Value::Flag(value)) => value,
        _ => false,
    }
}

/// Idempotent install. Returns true on first install; a reconnect that
/// re-runs install gets false and registers nothing twice.
pub fn install() -> bool {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        // A reconnect can preserve the static registrations, but cached meter
        // ownership must never cross a transport/session boundary.
        meter_snapshot::clear();
        return false;
    }
    if let Err(error) = generated::register_all() {
        // Never leave a half-installed artifact armed. The generated adapter
        // is static, but a diagnostic is still preferable to a later panic.
        dispatcher::clear();
        INSTALLED.store(false, Ordering::SeqCst);
        crate::slight::diag::note(format!("generated gimmick registration rejected: {error}"));
        return false;
    }
    // This slot is independently managed but shares the runtime's install-once
    // boundary with behavior. Its generated `install` is a no-op when the
    // selected normal export has no ACMD edits, which clears stale prior
    // registrations without adding a second plugin.
    acmd_generated::install();
    live_parameters::install();
    damage_capture::install();
    true
}

pub fn is_installed() -> bool {
    INSTALLED.load(Ordering::SeqCst)
}

/// Fighter spawn or respawn: the new stock starts with no inherited state.
pub fn on_init_fighter(boid: u32) {
    damage_control::retire(boid);
    operations::clear_fighter_status(boid);
    cloud_resource::retire(boid);
    damage_feed::lifecycle();
    live_behavior::clear_boid(boid);
    // A fresh stock establishes its current status as a baseline.  It must
    // never inherit an edge from a prior object that reused this boid.
    crate::slight::agents::reset_status_observation(boid);
    state::reset_boid(boid);
    meter_snapshot::clear_boid(boid);
}

/// Fighter reset (KO, match exit): the dead object's state dies with it.
pub fn on_reset_fighter(boid: u32) {
    damage_control::retire(boid);
    operations::clear_fighter_status(boid);
    cloud_resource::retire(boid);
    damage_feed::lifecycle();
    live_behavior::clear_boid(boid);
    // Reset/teardown status changes are lifecycle bookkeeping, not behavior
    // triggers.  This also removes an edge sampled earlier in this global
    // frame before Event System dispatch reaches it.
    crate::slight::agents::reset_status_observation(boid);
    state::reset_boid(boid);
    meter_snapshot::clear_boid(boid);
}

/// Weapon despawn: article state dies with its object.
pub fn on_reset_weapon(boid: u32) {
    live_behavior::clear_boid(boid);
    state::reset_boid(boid);
    meter_snapshot::clear_boid(boid);
}

/// Match exit: discard every object-scoped value while retaining the static generated handler
/// registrations for the next match. Full [`clear`] is reserved for plugin/system uninstall;
/// using it here would leave subsequent matches without generated behavior.
pub fn on_match_end() {
    damage_control::configuration_changed();
    operations::clear_fighter_statuses();
    cloud_resource::reset_match();
    damage_feed::lifecycle();
    live_behavior::clear_all();
    state::reset_all();
    meter_snapshot::clear();
    damage_capture::reset_match();
}

/// One frame of hook firings.
pub fn on_frame() {
    if !is_installed() {
        return;
    }
    live_parameters::refresh_identities();
    cloud_resource::on_frame();
    dispatcher::pump();
    meter_snapshot::on_frame(generated::meter_bindings());
    damage_capture::on_frame();
    deflect_volume::tick();
}

/// Current generated Work-meter values for the desktop's read-only live
/// preview. The binding set is compile-time data owned by the managed slot.
pub fn meter_wire_rows() -> Vec<serde_json::Value> {
    meter_snapshot::wire_rows(generated::meter_bindings())
}

/// Observe one lifecycle event after the framework removes it from its queue.
/// Keeping observation here prevents gimmicks from becoming a competing queue
/// consumer whose behavior depends on facade ordering.
pub fn observe_event(event: &crate::slight::systems::event_system::GameEvent) {
    if is_installed() {
        match event {
            crate::slight::systems::event_system::GameEvent::FighterRebirthObserved { .. } => {
                damage_feed::lifecycle()
            }
            crate::slight::systems::event_system::GameEvent::FighterDownObserved { .. } => {
                damage_feed::lifecycle()
            }
            _ => {}
        }
        if let crate::slight::systems::event_system::GameEvent::StatusChange {
            boid,
            module_accessor_addr,
            fighter_kind,
            entry_id,
            stock_generation,
            ..
        } = event
        {
            if !crate::slight::agents::status_transition_is_current(
                *boid,
                *module_accessor_addr,
                *fighter_kind,
                *entry_id,
                *stock_generation,
            ) {
                return;
            }
        }
        if let crate::slight::systems::event_system::GameEvent::CloudLimitGroundedNeutralSpent {
            boid,
            fighter_kind,
            entry_id,
            stock_generation,
            module_accessor_addr,
        }
        | crate::slight::systems::event_system::GameEvent::CloudLimitIdleExpired {
            boid,
            fighter_kind,
            entry_id,
            stock_generation,
            module_accessor_addr,
        } = event
        {
            let valid = crate::slight::agents::lookup(*boid).is_some_and(|record| {
                record.category == 0
                    && record.kind == *fighter_kind
                    && record.entry_id == *entry_id
                    && record.module_accessor_addr == *module_accessor_addr
                    && crate::slight::agents::stock_generation(&record) == Some(*stock_generation)
            });
            if !valid {
                return;
            }
        }
        if let crate::slight::systems::event_system::GameEvent::FighterRebirthObserved {
            boid,
            ..
        } = event
        {
            // The tracker has already advanced this concrete identity's
            // generation. Clear generated state without asking it to forget
            // the freshly observed lifecycle proof.
            live_behavior::clear_stock(*boid);
            state::reset_boid(*boid);
            meter_snapshot::clear_boid(*boid);
        }
        dispatcher::observe_event(event);
    }
}

fn timeout_fired(boid: u32) {
    dispatcher::fire(dispatcher::GimmickHook::Timeout, boid);
}

/// Full teardown: handlers, state, and the install latch.
pub fn on_uninstall() {
    clear();
}

/// Wipe handlers and state and disarm the latch, for the systems-wide clear
/// path. Teardown clears state — a cleared system keeps nothing.
pub fn clear() {
    damage_control::configuration_changed();
    operations::clear_fighter_statuses();
    cloud_resource::clear();
    live_behavior::clear_overrides();
    live_behavior::clear_compiled_baseline();
    live_parameters::clear();
    dispatcher::clear();
    state::reset_all();
    meter_snapshot::clear();
    damage_capture::reset_match();
    INSTALLED.store(false, Ordering::SeqCst);
}

/// Serializes the unit tests: the registry and the store are process-global,
/// so parallel tests would wipe each other's entries mid-assertion. Held for
/// a whole test body; always outside the registry/store locks.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static STOCK_START_BOID: AtomicU32 = AtomicU32::new(0);

    fn record_stock_start(boid: u32) {
        let observed = if state::get(boid, "gauge").is_none() {
            boid
        } else {
            u32::MAX
        };
        STOCK_START_BOID.store(observed, Ordering::SeqCst);
    }

    #[test]
    fn install_is_idempotent_and_uninstall_disarms() {
        let _serial = super::TEST_LOCK.lock().unwrap();
        on_uninstall();
        assert!(install());
        assert!(is_installed());
        assert!(!install());
        assert!(is_installed());
        on_uninstall();
        assert!(!is_installed());
    }

    #[test]
    fn lifecycle_wipes_state_but_keeps_handlers_until_uninstall() {
        let _serial = super::TEST_LOCK.lock().unwrap();
        on_uninstall();
        install();
        fn handler(_: u32) {}
        dispatcher::register(dispatcher::GimmickHook::Ko, handler);
        state::set(21, "gauge", state::VarValue::Float(1.0));
        // KO reset wipes the dead object's state; the handler survives —
        // teardown clears state, not registrations.
        on_reset_fighter(21);
        assert_eq!(state::get(21, "gauge"), None);
        assert_eq!(dispatcher::handler_count(dispatcher::GimmickHook::Ko), 1);
        // Fresh spawn also starts clean.
        state::set(22, "gauge", state::VarValue::Float(1.0));
        on_init_fighter(22);
        assert_eq!(state::get(22, "gauge"), None);
        // Uninstall clears both, and a reconnect reinstalls exactly once.
        on_uninstall();
        assert_eq!(dispatcher::handler_count(dispatcher::GimmickHook::Ko), 0);
        assert!(install());
        assert!(!install());
        on_uninstall();
    }

    #[test]
    fn match_end_wipes_object_state_but_keeps_generated_handlers() {
        let _serial = super::TEST_LOCK.lock().unwrap();
        on_uninstall();
        install();
        fn handler(_: u32) {}
        dispatcher::register(dispatcher::GimmickHook::Ko, handler);
        state::set(24, "gauge", state::VarValue::Float(100.0));

        on_match_end();

        assert_eq!(state::get(24, "gauge"), None);
        assert_eq!(dispatcher::handler_count(dispatcher::GimmickHook::Ko), 1);
        assert!(is_installed());
        on_uninstall();
    }

    #[test]
    fn observed_rebirth_clears_state_before_firing_stock_start() {
        let _serial = super::TEST_LOCK.lock().unwrap();
        on_uninstall();
        install();
        STOCK_START_BOID.store(0, Ordering::SeqCst);
        dispatcher::register(dispatcher::GimmickHook::StockStart, record_stock_start);
        state::set(23, "gauge", state::VarValue::Float(100.0));

        observe_event(
            &crate::slight::systems::event_system::GameEvent::FighterRebirthObserved {
                boid: 23,
                fighter_kind: 62,
                entry_id: 0,
                stock_generation: 1,
            },
        );

        assert_eq!(state::get(23, "gauge"), None);
        assert_eq!(STOCK_START_BOID.load(Ordering::SeqCst), 23);
        on_uninstall();
    }

    #[test]
    fn on_frame_without_install_pumps_nothing() {
        let _serial = super::TEST_LOCK.lock().unwrap();
        on_uninstall();
        // No handlers, no install: must not deadlock or deliver.
        on_frame();
        assert!(!is_installed());
    }
}

fn controller_random(_seed: &std::cell::Cell<u64>, upper: i32) -> i32 {
    unsafe { smash::app::sv_math::rand(smash::hash40("fighter"), upper) }
}

#[path = "../../../../../src/gimmicks/damage_commands.rs"]
mod damage_commands;
pub(crate) mod damage_control;

#[path = "../../../../../src/gimmicks/status_model.rs"]
pub mod status_model;

#[path = "../../../../../src/gimmicks/status_commands.rs"]
pub mod status_commands;
pub mod status_runtime;
