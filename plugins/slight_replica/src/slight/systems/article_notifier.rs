//! Article / weapon spawn notifications — Jorge article_notifier facade.

use parking_lot::Mutex;
use smash::app::lua_bind::WorkModule;
use smash::app::sv_battle_object;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;

#[path = "article_lifetime.rs"]
mod article_lifetime;

use article_lifetime::{commit_lifetimes, lifetime_delta};

const WORK_OWNER_VAR: i32 = 0x10000000;

/// Weapons are keyed by both battle-object id and module accessor.  The game can
/// recycle a battle-object id, so the id alone is not a lifetime token.
static LIVE: LazyLock<Mutex<HashMap<u32, u64>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
#[derive(Clone)]
struct OwnedArticle {
    owner: u32,
    identity: crate::slight::gimmicks::actor_identity::Identity,
    symbol: &'static str,
}
static OWNED: LazyLock<Mutex<HashMap<u32, OwnedArticle>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn article_symbol(owner: u32, weapon: u32, fighter: &str) -> Option<&'static str> {
    use crate::slight::gimmicks::registry::{self, IdentifierKind};
    use smash::app::lua_bind::{Article, ArticleModule};
    unsafe {
        let accessor = sv_battle_object::module_accessor(owner);
        if accessor.is_null() {
            return None;
        }
        for entry in registry::ENTRIES.iter().filter(|entry| {
            entry.fighter == fighter
                && entry.kind == IdentifierKind::Article
                && entry.group.is_empty()
        }) {
            let count = ArticleModule::get_active_num(accessor, entry.value as i32);
            for index in 0..count {
                let article =
                    ArticleModule::get_article_from_no(accessor, entry.value as i32, index);
                if !article.is_null() && Article::get_battle_object_id(article) == u64::from(weapon)
                {
                    return Some(entry.symbol);
                }
            }
        }
    }
    None
}

fn emit_owned(article: &OwnedArticle, spawned: bool) {
    crate::slight::systems::event_system::emit(
        crate::slight::systems::event_system::GameEvent::OwnedArticle {
            owner_boid: article.owner,
            fighter_kind: article.identity.fighter_kind,
            entry_id: article.identity.entry_id,
            module_accessor_addr: article.identity.module_accessor_addr,
            stock_generation: article.identity.stock_generation,
            symbol: article.symbol,
            spawned,
        },
    );
}

static POLL_TICKS: AtomicU64 = AtomicU64::new(0);

pub fn install() {
    skyline::println!("[SLight] Article notifier ready");
}

/// Jorge article_notifier — resolve Pokemon Trainer / nested article owner slot.
/// Resolve a weapon's current fighter owner through the same checked path used
/// by article lifecycle notifications. Callers still validate the returned
/// fighter object before taking an engine action.
pub fn weapon_owner_boid(boid: u32) -> u32 {
    let resolved = crate::slight::frame_context::resolve_work_boid(boid);
    unsafe {
        let founder = sv_battle_object::get_founder_id(boid);
        if let Some(owner) = crate::slight::agents::lookup_by_founder(founder) {
            return owner.boid;
        }
        let ptr = sv_battle_object::module_accessor(boid);
        if !ptr.is_null() {
            let owner = WorkModule::get_int(ptr, WORK_OWNER_VAR) as u32;
            if sv_battle_object::is_active(owner) {
                return owner; // full battle object id
            }
        }
    }
    resolved
}

/// Return whether a resolved owner is the exact live fighter record that can
/// consume an ArticleSpawn event.  In particular, never fall back to the
/// article/weapon boid: live_behavior deliberately rejects non-fighters, and
/// sending that id here loses the notification instead of merely delaying it.
fn live_fighter_owner(weapon_boid: u32, owner_boid: u32) -> bool {
    if weapon_boid == owner_boid {
        return false;
    }
    let Some(record) = crate::slight::agents::lookup(owner_boid) else {
        return false;
    };
    if record.category != 0
        || record.kind < 0
        || record.entry_id < 0
        || record.module_accessor_addr == 0
    {
        return false;
    }
    unsafe {
        if sv_battle_object::is_null(owner_boid)
            || !sv_battle_object::is_active(owner_boid)
            || sv_battle_object::category(owner_boid) != 0
        {
            return false;
        }
        let accessor = sv_battle_object::module_accessor(owner_boid);
        !accessor.is_null() && accessor as u64 == record.module_accessor_addr
    }
}

fn live_weapon_record(record: &crate::slight::agents::AgentRecord) -> bool {
    if record.category != 1 || record.module_accessor_addr == 0 {
        return false;
    }
    unsafe {
        if sv_battle_object::is_null(record.boid)
            || !sv_battle_object::is_active(record.boid)
            || sv_battle_object::category(record.boid) != 1
        {
            return false;
        }
        let accessor = sv_battle_object::module_accessor(record.boid);
        !accessor.is_null() && accessor as u64 == record.module_accessor_addr
    }
}

pub fn on_weapon_frame() {
    // Weapons enter the registry via their smashline line callbacks / effect-spawn hooks;
    // full battle object ids can't be enumerated by scanning 0..512.
    let records = crate::slight::agents::all_records();
    let observed_weapons = records.iter().filter(|rec| rec.category == 1).count();
    let current: HashMap<u32, u64> = records
        .into_iter()
        .filter(live_weapon_record)
        .map(|rec| (rec.boid, rec.module_accessor_addr))
        .collect();

    let mut live = LIVE.lock();
    let mut owner_rejections = 0usize;
    let mut new_candidates = 0usize;
    let mut emitted = 0usize;
    let delta = lifetime_delta(&live, &current);
    // Treat an accessor change under the same boid as a despawn/rebirth.  This
    // prevents stale weapon module state from surviving object-id recycling.
    for &gone in &delta.reset {
        let old = OWNED.lock().remove(&gone);
        if let Some(old) = old {
            emit_owned(&old, false);
        }
        crate::slight::systems::main_module::on_reset_weapon(gone);
        crate::slight::systems::init_frame::clear_boid(gone);
        if crate::slight::smash_utils::debug_logging_enabled() {
            skyline::println!("[SLight] Article notifier: weapon {gone} despawned");
        }
    }

    let mut accepted = Vec::new();
    for &(boid, accessor) in &delta.candidates {
        new_candidates += 1;
        let owner = weapon_owner_boid(boid);
        if !live_fighter_owner(boid, owner) {
            owner_rejections += 1;
            // Do not mark an unresolved article live.  Ownership can become
            // observable on the next frame after the weapon callback, and the
            // notifier must retry rather than permanently dropping the edge.
            continue;
        }
        if crate::slight::systems::init_frame::facade_allowed("Main module", owner) {
            crate::slight::systems::main_module::on_init_weapon(boid);
            let owned = crate::slight::agents::lookup(owner)
                .and_then(|record| crate::slight::gimmicks::registry::fighter_name(record.kind))
                .and_then(|fighter| {
                    let identity =
                        crate::slight::gimmicks::actor_identity::Identity::from_live_fighter(
                            owner, fighter,
                        )?;
                    let symbol = article_symbol(owner, boid, fighter)?;
                    Some(OwnedArticle {
                        owner,
                        identity,
                        symbol,
                    })
                });
            if let Some(owned) = owned {
                OWNED.lock().insert(boid, owned.clone());
                emit_owned(&owned, true);
            } else {
                crate::slight::systems::event_system::emit(
                    crate::slight::systems::event_system::GameEvent::ArticleSpawn {
                        owner_boid: owner,
                        article_boid: boid,
                    },
                );
            }
            accepted.push((boid, accessor));
            emitted += 1;
            if crate::slight::smash_utils::debug_logging_enabled() {
                skyline::println!(
                    "[SLight] Article notifier: new weapon boid {boid} (owner {owner})"
                );
            }
        }
    }

    let poll = POLL_TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    if crate::slight::smash_utils::debug_logging_enabled() && poll % 60 == 0 {
        crate::slight::diag::note(format!(
            "[SLight] ARTICLE_NOTIFIER_DIAG poll={poll} observed_weapons={observed_weapons} current_weapons={} new_candidates={new_candidates} owner_rejections={owner_rejections} emitted={emitted}",
            current.len()
        ));
    }
    *live = commit_lifetimes(&delta, &accepted);
}

/// Live weapons this fighter owns of one article kind, matched by the
/// article's weapon kind (FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL ->
/// WEAPON_KIND_MARIO_FIREBALL). ArticleModule's per-article getters return
/// null for live fireballs, so they are not used here.
pub fn owned_weapons(owner: u32, fighter: &str, symbol: &str) -> Vec<u32> {
    let Some(group) = symbol
        .rsplit_once("_GENERATE_ARTICLE_")
        .map(|(_, g)| g.to_ascii_lowercase())
    else {
        return Vec::new();
    };
    let Some(kind) = crate::slight::gimmicks::registry::weapon_kind(fighter, &group) else {
        return Vec::new();
    };
    crate::slight::agents::all_records()
        .into_iter()
        .filter(|rec| rec.category == 1 && rec.kind == kind.value as i32 && live_weapon_record(rec))
        .filter(|rec| weapon_owner_boid(rec.boid) == owner)
        .map(|rec| rec.boid)
        .collect()
}

pub fn clear() {
    LIVE.lock().clear();
    OWNED.lock().clear();
    POLL_TICKS.store(0, Ordering::Relaxed);
}
