//! Main-hand presence edges on the owning fighter Main line.
use super::{actor_identity::Identity, native_inputs::ItemState};
use smash::app::{lua_bind::ItemModule, BattleObjectModuleAccessor};
use std::{collections::BTreeMap, sync::LazyLock};

// Keep the revision and rows under one lock so configuration replacement cannot
// seed a new configuration with an old sample. Native calls never hold it.
static ITEMS: LazyLock<parking_lot::Mutex<(u64, BTreeMap<u32, ItemState<Identity>>)>> =
    LazyLock::new(|| parking_lot::Mutex::new((0, BTreeMap::new())));

pub(crate) fn configuration_changed() {
    let mut items = ITEMS.lock();
    items.0 = items.0.wrapping_add(1);
    items.1.clear();
}
pub(crate) fn retire(boid: u32) {
    ITEMS.lock().1.remove(&boid);
}
pub(crate) fn observe(boid: u32) {
    if !super::is_installed() {
        return;
    }
    let Some(record) = crate::slight::agents::lookup(boid) else {
        return;
    };
    let Some(fighter) = super::registry::fighter_name(record.kind) else {
        return;
    };
    let Some(identity) = Identity::from_live_fighter(boid, fighter) else {
        retire(boid);
        return;
    };
    if !(0..8).contains(&identity.entry_id) {
        return;
    }
    let revision = ITEMS.lock().0;
    let holding = unsafe {
        ItemModule::is_have_item(
            identity.module_accessor_addr as *mut BattleObjectModuleAccessor,
            0,
        )
    };
    if Identity::from_live_fighter(boid, fighter) != Some(identity) {
        retire(boid);
        return;
    }
    let next = ItemState {
        identity,
        revision,
        holding,
    };
    let edges = {
        let mut items = ITEMS.lock();
        if items.0 != revision || (items.1.len() >= 128 && !items.1.contains_key(&boid)) {
            return;
        }
        items
            .1
            .insert(boid, next)
            .map_or([false; 2], |old| old.edges(&next))
    };
    for (edge, hook) in edges.into_iter().zip([
        super::dispatcher::GimmickHook::ItemPickup,
        super::dispatcher::GimmickHook::ItemThrown,
    ]) {
        if edge
            && ITEMS.lock().0 == revision
            && Identity::from_live_fighter(boid, fighter) == Some(identity)
        {
            super::dispatcher::fire(hook, boid);
        }
    }
}
