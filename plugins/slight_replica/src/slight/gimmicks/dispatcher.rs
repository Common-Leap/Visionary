//! Gimmick hook dispatcher (slice F2.4).
//!
//! Routes static-runtime hook firings to registered handlers. The desktop
//! capability registry is stricter than this internal vocabulary: it permits
//! emitted source only when a live, evidenced producer and the required event
//! payload exist. Retained variants therefore do not imply that a saved graph
//! can register them. Handlers are plain `fn(u32)` functions of the object id;
//! data reads happen through the systems at call time, keeping this module
//! free of engine calls and safe to unit test.
//!
//! [`pump`] translates one frame of engine feeds (event queue, collision
//! drains) into firings. Registration deduplicates: registering the same
//! function twice returns the existing id instead of doubling the call, so a
//! reconnect that re-runs install cannot duplicate handlers.

use parking_lot::Mutex;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;

use crate::slight::systems::event_system;

/// Internal firings the static runtime can route. The host planner decides
/// which of these have a usable gameplay producer before emitting source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GimmickHook {
    DealDamage,
    TakeDamage,
    AttackShielded,
    ShieldHit,
    Grab,
    Throw,
    Grabbed,
    ItemPickup,
    ItemThrown,
    ChargeTick,
    Spend,
    StatusEnter,
    StatusExit,
    /// Exact grounded Limit Blade Beam consumption, not a generic status or gauge-drop event.
    CloudLimitGroundedNeutralSpend,
    /// Native Limit timer expiry while both samples use reviewed standing-idle motions.
    CloudLimitIdleExpiry,
    Timeout,
    Switch,
    StockStart,
    Ko,
    ArticleSpawn,
    ArticleRemoved,
}

/// One hook firing for one object: the unit [`pump`] produces and [`fire`]
/// delivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Firing {
    pub hook: GimmickHook,
    pub boid: u32,
}

/// A hook handler: receives the object the hook fired for.
pub type Handler = fn(u32);

/// Registration token for [`unregister`].
pub type HandlerId = u64;

struct Registry {
    next: HandlerId,
    handlers: BTreeMap<GimmickHook, Vec<(HandlerId, Handler)>>,
    status_handlers: BTreeMap<(GimmickHook, i32), Vec<(HandlerId, Handler)>>,
}

impl Registry {
    fn new() -> Self {
        Self {
            next: 1,
            handlers: BTreeMap::new(),
            status_handlers: BTreeMap::new(),
        }
    }
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(Registry::new()));
static LIMIT_CONSUMER: AtomicBool = AtomicBool::new(false);

/// Register a handler. Registering the same function on the same hook twice
/// returns the existing id — reconnect-safe by construction.
pub fn register(hook: GimmickHook, handler: Handler) -> HandlerId {
    let mut registry = REGISTRY.lock();
    if matches!(
        hook,
        GimmickHook::CloudLimitGroundedNeutralSpend | GimmickHook::CloudLimitIdleExpiry
    ) {
        LIMIT_CONSUMER.store(true, Ordering::Relaxed);
    }
    if let Some((id, _)) = registry
        .handlers
        .get(&hook)
        .into_iter()
        .flatten()
        .find(|(_, known)| *known as usize == handler as usize)
    {
        return *id;
    }
    let id = registry.next;
    registry.next += 1;
    registry
        .handlers
        .entry(hook)
        .or_default()
        .push((id, handler));
    id
}

/// Register a handler for one exact status edge. A status-scoped handler is
/// never called by [`fire`]; only a matching `GameEvent::StatusChange` can
/// reach it. The full `(hook, status, function)` tuple is deduplicated so two
/// different reviewed statuses may intentionally share one handler.
pub fn register_status(hook: GimmickHook, status: i32, handler: Handler) -> HandlerId {
    let mut registry = REGISTRY.lock();
    let key = (hook, status);
    if let Some((id, _)) = registry
        .status_handlers
        .get(&key)
        .into_iter()
        .flatten()
        .find(|(_, known)| *known as usize == handler as usize)
    {
        return *id;
    }
    let id = registry.next;
    registry.next += 1;
    registry
        .status_handlers
        .entry(key)
        .or_default()
        .push((id, handler));
    id
}

/// Remove one registration. `false` is an unknown id, never an error.
pub fn unregister(id: HandlerId) -> bool {
    let mut registry = REGISTRY.lock();
    let mut removed = false;
    for handlers in registry.handlers.values_mut() {
        let before = handlers.len();
        handlers.retain(|(known, _)| *known != id);
        removed |= handlers.len() != before;
    }
    for handlers in registry.status_handlers.values_mut() {
        let before = handlers.len();
        handlers.retain(|(known, _)| *known != id);
        removed |= handlers.len() != before;
    }
    LIMIT_CONSUMER.store(
        [
            GimmickHook::CloudLimitGroundedNeutralSpend,
            GimmickHook::CloudLimitIdleExpiry,
        ]
        .iter()
        .any(|hook| {
            registry
                .handlers
                .get(hook)
                .is_some_and(|handlers| !handlers.is_empty())
        }),
        Ordering::Relaxed,
    );
    removed
}

/// Handlers registered on one hook, for diagnostics and tests.
pub fn handler_count(hook: GimmickHook) -> usize {
    let registry = REGISTRY.lock();
    registry
        .handlers
        .get(&hook)
        .map(|handlers| handlers.len())
        .unwrap_or(0)
        + registry
            .status_handlers
            .iter()
            .filter(|((known, _), _)| *known == hook)
            .map(|(_, handlers)| handlers.len())
            .sum::<usize>()
}

/// Deliver one firing to every handler on its hook. The handler list is
/// copied under the lock and called outside it, so a handler that registers
/// or unregisters cannot deadlock the dispatcher.
pub fn fire(hook: GimmickHook, boid: u32) {
    let handlers: Vec<Handler> = REGISTRY
        .lock()
        .handlers
        .get(&hook)
        .map(|handlers| handlers.iter().map(|(_, handler)| *handler).collect())
        .unwrap_or_default();
    for handler in handlers {
        handler(boid);
    }
    super::live_behavior::dispatch(hook, boid);
}

/// Deliver one exact status-bound firing. Kept separate from [`fire`] so a
/// generic producer cannot accidentally trigger a semantic status hook.
fn fire_status(
    hook: GimmickHook,
    status: i32,
    boid: u32,
    identity_is_current: &mut impl FnMut() -> bool,
) -> bool {
    let handlers: Vec<Handler> = REGISTRY
        .lock()
        .status_handlers
        .get(&(hook, status))
        .map(|handlers| handlers.iter().map(|(_, handler)| *handler).collect())
        .unwrap_or_default();
    for handler in handlers {
        if !identity_is_current() {
            return false;
        }
        handler(boid);
    }
    // Static handlers run outside the registry lock. One may reset or replace
    // this BOID, so revalidate the original captured edge before its live route.
    if !identity_is_current() {
        return false;
    }
    match hook {
        GimmickHook::Spend => super::live_behavior::dispatch_status(status, boid),
        GimmickHook::StatusEnter => super::live_behavior::dispatch_status_enter(status, boid),
        GimmickHook::StatusExit => super::live_behavior::dispatch_status_exit(status, boid),
        _ => {}
    }
    true
}

fn observe_status_change_with(
    boid: u32,
    previous_status: i32,
    status: i32,
    identity_is_current: &mut impl FnMut() -> bool,
) -> bool {
    if !fire_status(
        GimmickHook::StatusExit,
        previous_status,
        boid,
        identity_is_current,
    ) {
        return false;
    }
    if !fire_status(GimmickHook::Spend, status, boid, identity_is_current) {
        return false;
    }
    fire_status(GimmickHook::StatusEnter, status, boid, identity_is_current)
}

/// Deliver a batch of firings in order.
pub fn fire_all(firings: &[Firing]) {
    for firing in firings {
        fire(firing.hook, firing.boid);
    }
}

/// Drop every registration: match teardown and uninstall.
pub fn clear() {
    *REGISTRY.lock() = Registry::new();
    LIMIT_CONSUMER.store(false, Ordering::Relaxed);
    super::damage_dispatch::clear();
}

/// Translate one game event into a firing. Initial Spawn and the exact,
/// identity-aware REBIRTH edge fire stock-start; the legacy Respawn
/// notification is deliberately ignored because magic/object reinit is not a
/// proven stock boundary. Article spawn fires against its resolved owner, and
/// the exact fighter-down observation fires KO with cause-agnostic lost-stock
/// semantics. Legacy Die/Kills/Suicide events map to the same hook. Everything
/// else translates to nothing. Status changes are routed separately against exact registrations;
/// they never become an unscoped firing. Pure and total, so tests pin the
/// mapping without the game.
pub fn translate_event(event: &event_system::GameEvent) -> Option<Firing> {
    use event_system::GameEvent as E;
    match event {
        E::Spawn { boid } | E::FighterRebirthObserved { boid, .. } => Some(Firing {
            hook: GimmickHook::StockStart,
            boid: *boid,
        }),
        E::Die { boid }
        | E::Kills { boid }
        | E::Suicide { boid }
        | E::FighterDownObserved { boid, .. } => Some(Firing {
            hook: GimmickHook::Ko,
            boid: *boid,
        }),
        E::ArticleSpawn { owner_boid, .. } => Some(Firing {
            hook: GimmickHook::ArticleSpawn,
            boid: *owner_boid,
        }),
        E::CloudLimitGroundedNeutralSpent { boid, .. } => Some(Firing {
            hook: GimmickHook::CloudLimitGroundedNeutralSpend,
            boid: *boid,
        }),
        E::CloudLimitIdleExpired { boid, .. } => Some(Firing {
            hook: GimmickHook::CloudLimitIdleExpiry,
            boid: *boid,
        }),
        _ => None,
    }
}

/// Observe a framework event without consuming its shared queue.
pub fn observe_event(event: &event_system::GameEvent) {
    if let event_system::GameEvent::OwnedArticle {
        owner_boid,
        fighter_kind,
        entry_id,
        module_accessor_addr,
        stock_generation,
        symbol,
        spawned,
    } = event
    {
        let Some(fighter) = super::registry::fighter_name(*fighter_kind) else {
            return;
        };
        let expected = super::actor_identity::Identity {
            fighter_kind: *fighter_kind,
            entry_id: *entry_id,
            module_accessor_addr: *module_accessor_addr,
            stock_generation: *stock_generation,
        };
        if super::actor_identity::Identity::from_live_fighter(*owner_boid, fighter)
            != Some(expected)
            || super::registry::resolve(fighter, symbol, super::registry::IdentifierKind::Article)
                .is_err()
        {
            return;
        }
        super::article_context::with_event(*owner_boid, expected, *spawned, symbol, || {
            fire(
                if *spawned {
                    GimmickHook::ArticleSpawn
                } else {
                    GimmickHook::ArticleRemoved
                },
                *owner_boid,
            )
        });
        return;
    }
    if let event_system::GameEvent::StatusChange {
        boid,
        module_accessor_addr,
        fighter_kind,
        entry_id,
        stock_generation,
        previous_status,
        status,
    } = event
    {
        let mut identity_is_current = || {
            crate::slight::agents::status_transition_is_current(
                *boid,
                *module_accessor_addr,
                *fighter_kind,
                *entry_id,
                *stock_generation,
            )
        };
        let _ =
            observe_status_change_with(*boid, *previous_status, *status, &mut identity_is_current);
    }
    if let Some(firing) = translate_event(event) {
        fire(firing.hook, firing.boid);
    }
}

/// One frame of frame-local engine feeds translated into firings.
///
/// Attacker-side collision records fire deal-damage for their attacker;
/// defender-side records fire take-damage for their defender. The
/// article-overload queue is deliberately skipped: article hits already
/// appear in the attacker queue, and firing both would double-count them.
pub fn pump() {
    use crate::slight::systems::damage_manager;

    let mut firings = Vec::new();
    for hit in damage_manager::drain_collision_hits() {
        firings.push(Firing {
            hook: GimmickHook::DealDamage,
            boid: hit.attacker_boid,
        });
    }
    for hit in damage_manager::drain_defender_hits() {
        firings.push(Firing {
            hook: GimmickHook::TakeDamage,
            boid: hit.defender_boid,
        });
    }
    // Charge tick is a per-fighter hook, not one anonymous global tick.
    for fighter in crate::slight::agents::all_records()
        .into_iter()
        .filter(|record| record.category == 0)
    {
        firings.push(Firing {
            hook: GimmickHook::ChargeTick,
            boid: fighter.boid,
        });
    }
    // Deterministic delivery order: damage before lifecycle, attackers before
    // defenders — the drains above already append in that order.
    fire_all(&firings);
}

/// Hooks currently holding handlers, for diagnostics and tests.
pub fn armed_hooks() -> BTreeSet<GimmickHook> {
    let registry = REGISTRY.lock();
    registry
        .handlers
        .iter()
        .filter(|(_, handlers)| !handlers.is_empty())
        .map(|(hook, _)| *hook)
        .chain(
            registry
                .status_handlers
                .iter()
                .filter(|(_, handlers)| !handlers.is_empty())
                .map(|((hook, _), _)| *hook),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    static SEEN_A: AtomicU32 = AtomicU32::new(0);
    static SEEN_B: AtomicU32 = AtomicU32::new(0);
    static IDENTITY_CURRENT: AtomicBool = AtomicBool::new(true);

    fn seen_a(boid: u32) {
        SEEN_A.store(boid, Ordering::SeqCst);
    }

    fn seen_b(boid: u32) {
        SEEN_B.store(boid, Ordering::SeqCst);
    }

    fn invalidate_identity(_: u32) {
        IDENTITY_CURRENT.store(false, Ordering::SeqCst);
    }

    fn route_status_for_test(boid: u32, previous_status: i32, status: i32) -> bool {
        let mut current = || true;
        observe_status_change_with(boid, previous_status, status, &mut current)
    }

    fn isolated() {
        clear();
        SEEN_A.store(0, Ordering::SeqCst);
        SEEN_B.store(0, Ordering::SeqCst);
        IDENTITY_CURRENT.store(true, Ordering::SeqCst);
    }

    fn status_event(boid: u32, status: i32) -> event_system::GameEvent {
        event_system::GameEvent::StatusChange {
            boid,
            module_accessor_addr: 0,
            fighter_kind: 62,
            entry_id: 0,
            previous_status: status - 1,
            status,
            stock_generation: 0,
        }
    }

    #[test]
    fn firing_calls_every_handler_on_its_hook_only() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        isolated();
        register(GimmickHook::DealDamage, seen_a);
        register(GimmickHook::DealDamage, seen_b);
        register(GimmickHook::Ko, seen_b);
        fire(GimmickHook::DealDamage, 7);
        assert_eq!(SEEN_A.load(Ordering::SeqCst), 7);
        assert_eq!(SEEN_B.load(Ordering::SeqCst), 7);
        SEEN_B.store(0, Ordering::SeqCst);
        fire(GimmickHook::Ko, 9);
        assert_eq!(SEEN_A.load(Ordering::SeqCst), 7);
        assert_eq!(SEEN_B.load(Ordering::SeqCst), 9);
        isolated();
    }

    #[test]
    fn registering_twice_returns_one_id_and_fires_once() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        isolated();
        static COUNT: AtomicU32 = AtomicU32::new(0);
        fn counting(_: u32) {
            COUNT.fetch_add(1, Ordering::SeqCst);
        }
        COUNT.store(0, Ordering::SeqCst);
        let first = register(GimmickHook::ChargeTick, counting);
        let second = register(GimmickHook::ChargeTick, counting);
        assert_eq!(first, second);
        assert_eq!(handler_count(GimmickHook::ChargeTick), 1);
        fire(GimmickHook::ChargeTick, 3);
        assert_eq!(COUNT.load(Ordering::SeqCst), 1);
        // Same function on a different hook is a separate registration.
        register(GimmickHook::Timeout, counting);
        assert_eq!(handler_count(GimmickHook::Timeout), 1);
        isolated();
    }

    #[test]
    fn status_registration_fires_only_for_its_exact_status() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        isolated();
        let first = register_status(GimmickHook::Spend, 0x1A2, seen_a);
        let duplicate = register_status(GimmickHook::Spend, 0x1A2, seen_a);
        assert_eq!(first, duplicate);
        assert_eq!(handler_count(GimmickHook::Spend), 1);

        route_status_for_test(44, 0x1A0, 0x1A1);
        assert_eq!(SEEN_A.load(Ordering::SeqCst), 0);
        route_status_for_test(44, 0x1A1, 0x1A2);
        assert_eq!(SEEN_A.load(Ordering::SeqCst), 44);

        // The same handler on another reviewed status is distinct.
        let second = register_status(GimmickHook::Spend, 0x1A3, seen_a);
        assert_ne!(first, second);
        assert_eq!(handler_count(GimmickHook::Spend), 2);
        assert!(unregister(first));
        assert_eq!(handler_count(GimmickHook::Spend), 1);
        clear();
        assert_eq!(handler_count(GimmickHook::Spend), 0);
        assert!(!armed_hooks().contains(&GimmickHook::Spend));
    }

    #[test]
    fn status_enter_registration_is_exact_and_distinct_from_spend() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        isolated();
        register_status(GimmickHook::StatusEnter, 0x1A2, seen_b);
        route_status_for_test(45, 0x1A0, 0x1A1);
        assert_eq!(SEEN_B.load(Ordering::SeqCst), 0);
        route_status_for_test(45, 0x1A1, 0x1A2);
        assert_eq!(SEEN_B.load(Ordering::SeqCst), 45);
        assert!(armed_hooks().contains(&GimmickHook::StatusEnter));
        isolated();
    }

    #[test]
    fn status_exit_registration_matches_only_the_previous_status() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        isolated();
        register_status(GimmickHook::StatusExit, 0x1A1, seen_b);
        route_status_for_test(46, 0x1A1, 0x1A2);
        assert_eq!(SEEN_B.load(Ordering::SeqCst), 46);
        SEEN_B.store(0, Ordering::SeqCst);
        register_status(GimmickHook::StatusExit, 0x1A2, seen_a);
        route_status_for_test(46, 0x1A1, 0x1A2);
        assert_eq!(SEEN_A.load(Ordering::SeqCst), 0);
        isolated();
    }

    #[test]
    fn one_status_edge_routes_exit_from_a_and_entry_into_b_once_each() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        isolated();
        register_status(GimmickHook::StatusExit, 0x1A1, seen_a);
        register_status(GimmickHook::StatusEnter, 0x1A2, seen_b);

        route_status_for_test(47, 0x1A1, 0x1A2);
        assert_eq!(SEEN_A.load(Ordering::SeqCst), 47);
        assert_eq!(SEEN_B.load(Ordering::SeqCst), 47);

        SEEN_A.store(0, Ordering::SeqCst);
        SEEN_B.store(0, Ordering::SeqCst);
        route_status_for_test(47, 0x1A2, 0x1A3);
        assert_eq!(SEEN_A.load(Ordering::SeqCst), 0);
        assert_eq!(SEEN_B.load(Ordering::SeqCst), 0);
        isolated();
    }

    #[test]
    fn identity_invalidated_during_exit_aborts_entry_for_the_same_captured_edge() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        isolated();
        register_status(GimmickHook::StatusExit, 0x1A1, seen_a);
        register_status(GimmickHook::StatusExit, 0x1A1, invalidate_identity);
        register_status(GimmickHook::StatusEnter, 0x1A2, seen_b);
        let mut current = || IDENTITY_CURRENT.load(Ordering::SeqCst);

        assert!(!observe_status_change_with(48, 0x1A1, 0x1A2, &mut current,));
        assert_eq!(SEEN_A.load(Ordering::SeqCst), 48);
        assert_eq!(SEEN_B.load(Ordering::SeqCst), 0);
        isolated();
    }

    #[test]
    fn generic_fire_cannot_reach_a_status_registration() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        isolated();
        register_status(GimmickHook::Spend, 7, seen_a);
        assert!(armed_hooks().contains(&GimmickHook::Spend));
        fire(GimmickHook::Spend, 90);
        assert_eq!(SEEN_A.load(Ordering::SeqCst), 0);
        isolated();
    }

    #[test]
    fn unregister_removes_one_registration() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        isolated();
        let id = register(GimmickHook::StockStart, seen_a);
        assert!(unregister(id));
        assert!(!unregister(id));
        assert_eq!(handler_count(GimmickHook::StockStart), 0);
        fire(GimmickHook::StockStart, 5);
        assert_eq!(SEEN_A.load(Ordering::SeqCst), 0);
        isolated();
    }

    #[test]
    fn clear_disarms_every_hook() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        isolated();
        register(GimmickHook::DealDamage, seen_a);
        register(GimmickHook::Ko, seen_b);
        assert_eq!(armed_hooks().len(), 2);
        clear();
        assert!(armed_hooks().is_empty());
        fire(GimmickHook::DealDamage, 7);
        assert_eq!(SEEN_A.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn batches_deliver_in_order() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        isolated();
        register(GimmickHook::DealDamage, seen_a);
        register(GimmickHook::TakeDamage, seen_b);
        fire_all(&[
            Firing {
                hook: GimmickHook::DealDamage,
                boid: 11,
            },
            Firing {
                hook: GimmickHook::TakeDamage,
                boid: 12,
            },
        ]);
        assert_eq!(SEEN_A.load(Ordering::SeqCst), 11);
        assert_eq!(SEEN_B.load(Ordering::SeqCst), 12);
        isolated();
    }

    #[test]
    fn events_translate_to_lifecycle_firings_and_nothing_else() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        use event_system::GameEvent as E;
        assert_eq!(
            translate_event(&E::Spawn { boid: 1 }),
            Some(Firing {
                hook: GimmickHook::StockStart,
                boid: 1
            })
        );
        assert_eq!(
            translate_event(&E::FighterRebirthObserved {
                boid: 2,
                fighter_kind: 62,
                entry_id: 0,
                stock_generation: 1,
            }),
            Some(Firing {
                hook: GimmickHook::StockStart,
                boid: 2
            })
        );
        assert_eq!(translate_event(&E::Respawn { boid: 2 }), None);
        assert_eq!(
            translate_event(&E::ArticleSpawn {
                owner_boid: 8,
                article_boid: 80,
            }),
            Some(Firing {
                hook: GimmickHook::ArticleSpawn,
                boid: 8,
            })
        );
        for (index, event) in [
            E::Die { boid: 3 },
            E::Kills { boid: 4 },
            E::Suicide { boid: 5 },
        ]
        .iter()
        .enumerate()
        {
            assert_eq!(
                translate_event(event),
                Some(Firing {
                    hook: GimmickHook::Ko,
                    boid: 3 + index as u32
                }),
                "{event:?}"
            );
        }
        assert_eq!(
            translate_event(&E::FighterDownObserved {
                boid: 6,
                fighter_kind: 10,
                entry_id: 0,
                dead_reason: 4,
                ko_entry_id: 2,
                stock_generation: 0,
            }),
            Some(Firing {
                hook: GimmickHook::Ko,
                boid: 6,
            })
        );
        // Status changes and everything else translate to nothing: no
        // binding, no firing.
        for event in [
            status_event(6, 10),
            E::RealAnimationChange { boid: 6 },
            E::Win { entry_id: 0 },
            E::StartsFinalAnim { boid: 6 },
            E::Loops { boid: 6 },
            E::Point { host: 0, port: 0 },
        ] {
            assert_eq!(translate_event(&event), None, "{event:?}");
        }
    }
}
