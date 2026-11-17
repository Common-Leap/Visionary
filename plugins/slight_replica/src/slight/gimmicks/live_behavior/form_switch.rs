//! Native form handoff for Aegis and Pokémon Trainer.

use super::*;

struct FormCarry {
    fighter: String,
    identity: Identity,
    saved: Option<(Arc<ControllerCoordinator>, ControllerState)>,
    age: u16,
}
static FORM_CARRY: LazyLock<Mutex<BTreeMap<i32, FormCarry>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

fn form_family(fighter: &str) -> Option<u8> {
    match fighter {
        "eflame" | "elight" => Some(0),
        "pzenigame" | "pfushigisou" | "plizardon" => Some(1),
        _ => None,
    }
}

type FormStatuses = BTreeMap<(i32, String), (Identity, i32)>;
static FORM_LAST_STATUS: LazyLock<Mutex<FormStatuses>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
pub(crate) fn clear_form_entry(entry: i32) {
    FORM_CARRY.lock().remove(&entry);
    FORM_LAST_STATUS.lock().retain(|(id, _), _| *id != entry);
}
pub(crate) fn clear_forms() {
    FORM_CARRY.lock().clear();
    FORM_LAST_STATUS.lock().clear();
}

pub(crate) fn observe_form(boid: u32, fighter: &str) {
    if form_family(fighter).is_none() {
        return;
    }
    let Some(identity) = Identity::from_live_fighter(boid, fighter) else {
        return;
    };
    if !(0..8).contains(&identity.entry_id) {
        return;
    }
    let status = unsafe {
        smash::app::lua_bind::StatusModule::status_kind(
            identity.module_accessor_addr as *mut smash::app::BattleObjectModuleAccessor,
        )
    };
    use smash::lib::lua_const::*;
    let (standby, out) = match fighter {
        "eflame" => (
            *FIGHTER_EFLAME_STATUS_KIND_SPECIAL_LW_STANDBY,
            *FIGHTER_EFLAME_STATUS_KIND_SPECIAL_LW_OUT,
        ),
        "elight" => (
            *FIGHTER_ELIGHT_STATUS_KIND_SPECIAL_LW_STANDBY,
            *FIGHTER_ELIGHT_STATUS_KIND_SPECIAL_LW_OUT,
        ),
        "pzenigame" => (
            *FIGHTER_PZENIGAME_STATUS_KIND_SPECIAL_LW_STANDBY,
            *FIGHTER_PZENIGAME_STATUS_KIND_SPECIAL_LW_OUT,
        ),
        "pfushigisou" => (
            *FIGHTER_PFUSHIGISOU_STATUS_KIND_SPECIAL_LW_STANDBY,
            *FIGHTER_PFUSHIGISOU_STATUS_KIND_SPECIAL_LW_OUT,
        ),
        _ => (
            *FIGHTER_PLIZARDON_STATUS_KIND_SPECIAL_LW_STANDBY,
            *FIGHTER_PLIZARDON_STATUS_KIND_SPECIAL_LW_OUT,
        ),
    };
    let previous = FORM_LAST_STATUS
        .lock()
        .insert((identity.entry_id, fighter.into()), (identity, status));
    let mut pending = FORM_CARRY.lock();
    if let Some(old) = pending.get_mut(&identity.entry_id) {
        old.age = old.age.saturating_add(1);
        if old.age > 300 {
            pending.remove(&identity.entry_id);
        }
    }
    if status == *FIGHTER_STATUS_KIND_DEAD || status == *FIGHTER_STATUS_KIND_REBIRTH {
        pending.remove(&identity.entry_id);
        return;
    }
    if previous.is_none_or(|(old, _)| old != identity) {
        crate::slight::diag::note(format!(
            "FORM_OBJECT entry={} fighter={} identity={:?} status={}",
            identity.entry_id, fighter, identity, status
        ));
    }
    // Main also visits an inactive counterpart in WAIT. Consume the handoff
    // only once native activation enters the incoming form's OUT status.
    if status == out
        && pending.get(&identity.entry_id).is_some_and(|old| {
            old.fighter != fighter && form_family(&old.fighter) == form_family(fighter)
        })
    {
        let old = pending.remove(&identity.entry_id).unwrap();
        drop(pending);
        let result = (|| -> Result<(), String> {
            let (coordinator, source) = active_for_object(boid, fighter)?;
            let (context, mut state) = begin(boid, fighter, coordinator, source, None)?;
            if let Some((previous, previous_state)) = &old.saved {
                if active(&old.fighter).is_ok_and(|(current, _)| Arc::ptr_eq(&current, previous)) {
                    context.coordinator.carry_form_variables(
                        &mut state,
                        previous,
                        previous_state,
                    )?;
                }
            }
            context.commit(state)?;
            crate::slight::diag::note(format!(
                "FORM_SWITCH entry={} {} -> {} outgoing={:?} incoming={:?} status={}",
                identity.entry_id, old.fighter, fighter, old.identity, identity, status
            ));
            dispatch(model::HookName::OnSwitch, boid, fighter, None, None, None);
            Ok(())
        })();
        if let Err(error) = result {
            crate::slight::diag::note(format!("FORM_SWITCH rejected: {error}"));
        }
        return;
    }
    if status != *FIGHTER_STATUS_KIND_SPECIAL_LW && status != standby {
        return;
    }
    if status == *FIGHTER_STATUS_KIND_SPECIAL_LW
        && previous == Some((identity, status))
        && !pending
            .get(&identity.entry_id)
            .is_some_and(|old| old.identity == identity && old.fighter == fighter)
    {
        return;
    }
    if status == standby
        && !pending
            .get(&identity.entry_id)
            .is_some_and(|old| old.identity == identity && old.fighter == fighter)
    {
        return;
    }
    let saved = active_for_object(boid, fighter)
        .ok()
        .map(|(coordinator, source)| {
            let runtime = RUNTIME.lock();
            let object = match source {
                DispatchSource::Live { .. } => runtime.controller_objects.get(&boid),
                DispatchSource::Compiled => runtime.compiled_controller_objects.get(&boid),
                DispatchSource::Status { .. } => None,
            };
            let state = object
                .filter(|o| o.identity == identity)
                .map(|o| o.state.clone())
                .unwrap_or_else(|| coordinator.initial_state());
            (coordinator, state)
        });
    if !pending
        .get(&identity.entry_id)
        .is_some_and(|old| old.identity == identity && old.fighter == fighter)
    {
        crate::slight::diag::note(format!(
            "FORM_ARM entry={} fighter={} identity={:?} status={}",
            identity.entry_id, fighter, identity, status
        ));
    }
    let age = pending
        .get(&identity.entry_id)
        .filter(|old| old.identity == identity && old.fighter == fighter)
        .map_or(0, |old| old.age);
    pending.insert(
        identity.entry_id,
        FormCarry {
            fighter: fighter.into(),
            identity,
            saved,
            age,
        },
    );
}

pub(super) fn capture_committed(context: &DispatchContext, state: ControllerState) {
    if let Some(old) = FORM_CARRY.lock().get_mut(&context.identity.entry_id) {
        if old.identity == context.identity && old.fighter == context.fighter {
            old.saved = Some((Arc::clone(&context.coordinator), state));
        }
    }
}
