//! Match lifecycle events — Jorge event_system facade.

use parking_lot::Mutex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::LazyLock;

static EVENTS: LazyLock<Mutex<VecDeque<GameEvent>>> =
    LazyLock::new(|| Mutex::new(VecDeque::with_capacity(64)));
/// A bounded queue prevents a stalled consumer from growing unbounded during
/// a busy match. Eviction is intentionally oldest-first: events are ordered
/// observations, and a late status edge must never leapfrog older lifecycle
/// work to survive overflow.
const MAX_QUEUED_EVENTS: usize = 128;
static SEEN_FINAL: LazyLock<Mutex<HashSet<u32>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
static SEEN_WIN: LazyLock<Mutex<HashSet<i32>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// `(fighter_kind, EventKind) -> actions`. `fighter_kind == -1` = all kinds. This is the original
/// "Event System" facade's per-fighter-kind event→action registry ("Event map of fighter kind ",
/// "Pushing action for Event  of kind "); the registrations persist across matches.
static EVENT_REGISTRY: LazyLock<Mutex<HashMap<(i32, EventKind), Vec<EventAction>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone, Debug)]
pub enum GameEvent {
    RealAnimationChange {
        boid: u32,
    },
    Spawn {
        boid: u32,
    },
    OwnedArticle {
        owner_boid: u32,
        fighter_kind: i32,
        entry_id: i32,
        module_accessor_addr: u64,
        stock_generation: u32,
        symbol: &'static str,
        spawned: bool,
    },
    ArticleSpawn {
        owner_boid: u32,
        article_boid: u32,
    },
    Win {
        entry_id: i32,
    },
    Kills {
        boid: u32,
    },
    Die {
        boid: u32,
    },
    Suicide {
        boid: u32,
    },
    Respawn {
        boid: u32,
    },
    StatusChange {
        boid: u32,
        module_accessor_addr: u64,
        fighter_kind: i32,
        entry_id: i32,
        previous_status: i32,
        status: i32,
        stock_generation: u32,
    },
    /// Exact lost-stock edge sampled before reset/rebirth can clear identity
    /// and cause fields. Generated `onKO` intentionally has cause-agnostic
    /// stock-loss semantics; the metadata remains available for diagnostics.
    FighterDownObserved {
        boid: u32,
        fighter_kind: i32,
        entry_id: i32,
        dead_reason: i32,
        ko_entry_id: i32,
        stock_generation: u32,
    },
    /// Identity-aware transition into the engine's shared REBIRTH status.
    /// This is the production stock-start source after initial Spawn; the
    /// legacy Respawn notification is not precise enough to drive state.
    FighterRebirthObserved {
        boid: u32,
        fighter_kind: i32,
        entry_id: i32,
        stock_generation: u32,
    },
    /// Resource consumption sampled from adjacent frames of one exact Cloud stock/lifetime.
    CloudLimitGroundedNeutralSpent {
        boid: u32,
        fighter_kind: i32,
        entry_id: i32,
        stock_generation: u32,
        module_accessor_addr: u64,
    },
    /// Native timer expiry observed while the same Cloud lifetime stays standing idle.
    CloudLimitIdleExpired {
        boid: u32,
        fighter_kind: i32,
        entry_id: i32,
        stock_generation: u32,
        module_accessor_addr: u64,
    },
    StartsFinalAnim {
        boid: u32,
    },
    Loops {
        boid: u32,
    },
    Point {
        host: u16,
        port: u16,
    },
}

/// Event type (data-less) used as a registration key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EventKind {
    RealAnimationChange,
    Spawn,
    ArticleSpawn,
    OwnedArticle,
    Win,
    Kills,
    Die,
    Suicide,
    Respawn,
    StatusChange,
    FighterDownObserved,
    FighterRebirthObserved,
    CloudLimitGroundedNeutralSpent,
    CloudLimitIdleExpired,
    StartsFinalAnim,
    Loops,
    Point,
}

/// A registered reaction to an event (the original lets other mods register these per kind).
pub type EventAction = fn(&GameEvent);

impl GameEvent {
    fn event_kind(&self) -> EventKind {
        match self {
            GameEvent::RealAnimationChange { .. } => EventKind::RealAnimationChange,
            GameEvent::Spawn { .. } => EventKind::Spawn,
            GameEvent::OwnedArticle { .. } => EventKind::OwnedArticle,
            GameEvent::ArticleSpawn { .. } => EventKind::ArticleSpawn,
            GameEvent::Win { .. } => EventKind::Win,
            GameEvent::Kills { .. } => EventKind::Kills,
            GameEvent::Die { .. } => EventKind::Die,
            GameEvent::Suicide { .. } => EventKind::Suicide,
            GameEvent::Respawn { .. } => EventKind::Respawn,
            GameEvent::StatusChange { .. } => EventKind::StatusChange,
            GameEvent::FighterDownObserved { .. } => EventKind::FighterDownObserved,
            GameEvent::FighterRebirthObserved { .. } => EventKind::FighterRebirthObserved,
            GameEvent::CloudLimitGroundedNeutralSpent { .. } => {
                EventKind::CloudLimitGroundedNeutralSpent
            }
            GameEvent::CloudLimitIdleExpired { .. } => EventKind::CloudLimitIdleExpired,
            GameEvent::StartsFinalAnim { .. } => EventKind::StartsFinalAnim,
            GameEvent::Loops { .. } => EventKind::Loops,
            GameEvent::Point { .. } => EventKind::Point,
        }
    }

    fn fighter_kind(&self) -> Option<i32> {
        match self {
            GameEvent::OwnedArticle { fighter_kind, .. }
            | GameEvent::StatusChange { fighter_kind, .. }
            | GameEvent::FighterDownObserved { fighter_kind, .. }
            | GameEvent::FighterRebirthObserved { fighter_kind, .. }
            | GameEvent::CloudLimitGroundedNeutralSpent { fighter_kind, .. }
            | GameEvent::CloudLimitIdleExpired { fighter_kind, .. } => {
                // Use the identity captured with the edge. Looking this up
                // after queueing could observe a respawned or recycled object.
                return Some(*fighter_kind);
            }
            _ => {}
        }
        let boid = match self {
            GameEvent::RealAnimationChange { boid }
            | GameEvent::Spawn { boid }
            | GameEvent::Kills { boid }
            | GameEvent::Die { boid }
            | GameEvent::Suicide { boid }
            | GameEvent::Respawn { boid }
            | GameEvent::StartsFinalAnim { boid }
            | GameEvent::Loops { boid } => *boid,
            GameEvent::ArticleSpawn { owner_boid, .. }
            | GameEvent::OwnedArticle { owner_boid, .. } => *owner_boid,
            GameEvent::StatusChange { .. }
            | GameEvent::FighterDownObserved { .. }
            | GameEvent::FighterRebirthObserved { .. }
            | GameEvent::CloudLimitGroundedNeutralSpent { .. }
            | GameEvent::CloudLimitIdleExpired { .. } => {
                unreachable!("handled above")
            }
            GameEvent::Win { .. } | GameEvent::Point { .. } => return None,
        };
        crate::slight::agents::lookup(boid).map(|a| a.kind)
    }
}

/// Run any registered per-kind actions for a fired event (all-kinds `-1` first, then the fighter's
/// specific kind).
fn run_registered_actions(event: &GameEvent) {
    let ek = event.event_kind();
    let mut to_run: Vec<EventAction> = Vec::new();
    {
        let reg = EVENT_REGISTRY.lock();
        if let Some(actions) = reg.get(&(-1, ek)) {
            to_run.extend(actions.iter().copied());
        }
        if let Some(fk) = event.fighter_kind() {
            if let Some(actions) = reg.get(&(fk, ek)) {
                to_run.extend(actions.iter().copied());
            }
        }
    }
    for action in to_run {
        action(event);
    }
}

pub fn install() {
    skyline::println!("[SLight] Installing Event System");
    crate::slight::systems::win_screen::install();
    skyline::println!("[SLight] Event system installed");
}

pub fn emit(event: GameEvent) {
    // Keep push and capacity enforcement in one critical section. Status
    // lifecycle code deliberately nests this after AGENTS; event dispatch
    // drains before it asks AGENTS for fighter metadata, so no reverse lock
    // order exists.
    let mut events = EVENTS.lock();
    events.push_back(event);
    while events.len() > MAX_QUEUED_EVENTS {
        events.pop_front();
    }
}

pub fn drain() -> Vec<GameEvent> {
    EVENTS.lock().drain(..).collect()
}

/// Drop queued status/resource edges for a fighter whose lifecycle just reset. These
/// observations are sampled before the per-agent reset facade runs; retaining one
/// after that reset would turn teardown or respawn bookkeeping into a false
/// move transition.  Other events keep their original ordering and are not
/// touched.
pub fn discard_status_changes(boid: u32) {
    EVENTS.lock().retain(|event| {
        !matches!(event,
            GameEvent::StatusChange { boid: queued, .. }
            | GameEvent::CloudLimitGroundedNeutralSpent { boid: queued, .. }
            | GameEvent::CloudLimitIdleExpired { boid: queued, .. }
            if *queued == boid)
    });
}

fn dispatch(event: GameEvent) {
    // Gimmicks observe the event; they never drain this shared queue. Draining
    // from a second consumer would make delivery depend on facade order and
    // could suppress the framework's existing lifecycle handling.
    crate::slight::gimmicks::observe_event(&event);
    match &event {
        GameEvent::Win { entry_id } => {
            crate::slight::frame_context::set_after_win(true);
            if crate::slight::smash_utils::debug_logging_enabled() {
                skyline::println!(
                    "[SLight] IS_AFTER_WIN_SCREEN = true, due to check is after win screen and discovered on Win entry {entry_id}"
                );
            }
        }
        GameEvent::StartsFinalAnim { boid } => {
            crate::slight::frame_context::set_after_win(true);
            if crate::slight::smash_utils::debug_logging_enabled() {
                skyline::println!(
                    "[SLight] IS_AFTER_WIN_SCREEN = true, due to StartsFinalAnim on boid {boid}"
                );
            }
        }
        GameEvent::RealAnimationChange { boid } => {
            if crate::slight::smash_utils::debug_logging_enabled() {
                skyline::println!("[SLight] RealAnimationChange boid {boid}");
            }
        }
        GameEvent::Respawn { boid } => {
            // Notification-only. Scheduling a reinit here created an INFINITE feedback loop:
            // on_reinit_fighter emits Respawn → this scheduled another reinit → every frame,
            // forever, one loop added per KO (the "game slows down the more happens" bug —
            // ~10ms/frame of sequencer teardown/rebuild by mid-match). The reinit itself is
            // already performed by the init_frame poll (main_module facade) and gauge_system.
            if crate::slight::smash_utils::debug_logging_enabled() {
                skyline::println!("[SLight] Respawn event for boid {boid}");
            }
        }
        GameEvent::Die { boid } | GameEvent::Suicide { boid } => {
            crate::slight::effect_viewer::tracker::EFFECT_TRACKER
                .lock()
                .invalidate_boid(*boid);
        }
        GameEvent::FighterDownObserved {
            boid,
            fighter_kind,
            entry_id,
            dead_reason,
            ko_entry_id,
            stock_generation,
        } => {
            // Persist the complete pre-reset sample for the validation matrix.
            // This event deliberately has no generated-behavior side effect.
            crate::slight::diag::note(format!(
                "GIMMICK_FIGHTER_DOWN boid={boid:#x} kind={fighter_kind} entry={entry_id} reason={dead_reason} ko_entry={ko_entry_id} stock_generation={stock_generation}"
            ));
        }
        GameEvent::FighterRebirthObserved {
            boid,
            fighter_kind,
            entry_id,
            stock_generation,
        } => {
            if crate::slight::smash_utils::debug_logging_enabled() {
                skyline::println!(
                    "[SLight] Fighter rebirth boid={boid:#x} kind={fighter_kind} entry={entry_id} stock_generation={stock_generation}"
                );
            }
        }
        _ => {}
    }
    // Then run any per-kind actions registered through the Event System framework.
    run_registered_actions(&event);
    if crate::slight::smash_utils::debug_logging_enabled() {
        skyline::println!("[SLight] Event: {event:?}");
    }
}

pub fn on_frame() {
    // `win_screen`'s SD config re-read moved to the throttled tick (`slight::sd_poll`).
    for ev in drain() {
        dispatch(ev);
    }
    scan_final_animations();
}

fn scan_final_animations() {
    use smash::app::lua_bind::{MotionModule, StatusModule};
    use smash::app::sv_battle_object;

    for boid in 0..8u32 {
        unsafe {
            if !sv_battle_object::is_active(boid) || sv_battle_object::category(boid) != 0 {
                continue;
            }
            let ptr = sv_battle_object::module_accessor(boid);
            if ptr.is_null() {
                continue;
            }
            let motion = MotionModule::motion_kind(ptr);
            if crate::slight::systems::win_screen::is_final_motion(motion) {
                let mut seen = SEEN_FINAL.lock();
                if seen.insert(boid) {
                    drop(seen);
                    emit(GameEvent::StartsFinalAnim { boid });
                }
            }
            let status = StatusModule::status_kind(ptr);
            if crate::slight::systems::win_screen::is_win_status(status) {
                let entry = sv_battle_object::entry_id(boid);
                let mut seen = SEEN_WIN.lock();
                if seen.insert(entry) {
                    drop(seen);
                    emit(GameEvent::Win { entry_id: entry });
                }
            }
        }
    }
}

pub fn clear() {
    EVENTS.lock().clear();
    SEEN_FINAL.lock().clear();
    SEEN_WIN.lock().clear();
    crate::slight::systems::win_screen::clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_reset_discards_only_queued_status_and_resource_edges_for_that_boid() {
        let _serial = crate::slight::gimmicks::TEST_LOCK.lock().unwrap();
        EVENTS.lock().clear();
        emit(GameEvent::Spawn { boid: 1 });
        emit(GameEvent::StatusChange {
            boid: 1,
            module_accessor_addr: 0x1001,
            fighter_kind: 10,
            entry_id: 0,
            previous_status: 41,
            status: 42,
            stock_generation: 0,
        });
        emit(GameEvent::StatusChange {
            boid: 2,
            module_accessor_addr: 0x2002,
            fighter_kind: 11,
            entry_id: 1,
            previous_status: 42,
            status: 43,
            stock_generation: 0,
        });
        emit(GameEvent::Die { boid: 1 });
        emit(GameEvent::FighterDownObserved {
            boid: 1,
            fighter_kind: 10,
            entry_id: 0,
            dead_reason: 4,
            ko_entry_id: 2,
            stock_generation: 0,
        });
        emit(GameEvent::CloudLimitGroundedNeutralSpent {
            boid: 1,
            fighter_kind: 62,
            entry_id: 0,
            stock_generation: 0,
            module_accessor_addr: 0x1000,
        });
        emit(GameEvent::CloudLimitIdleExpired {
            boid: 1,
            fighter_kind: 62,
            entry_id: 0,
            stock_generation: 0,
            module_accessor_addr: 0x1000,
        });
        emit(GameEvent::CloudLimitIdleExpired {
            boid: 2,
            fighter_kind: 62,
            entry_id: 1,
            stock_generation: 0,
            module_accessor_addr: 0x2000,
        });

        discard_status_changes(1);
        let events = drain();
        assert_eq!(events.len(), 5);
        assert!(matches!(
            events[4],
            GameEvent::CloudLimitIdleExpired { boid: 2, .. }
        ));
        assert!(matches!(events[0], GameEvent::Spawn { boid: 1 }));
        assert!(matches!(
            events[1],
            GameEvent::StatusChange {
                boid: 2,
                status: 43,
                ..
            }
        ));
        assert!(matches!(events[2], GameEvent::Die { boid: 1 }));
        assert!(matches!(
            events[3],
            GameEvent::FighterDownObserved {
                boid: 1,
                stock_generation: 0,
                ..
            }
        ));
    }

    #[test]
    fn bounded_queue_evicts_oldest_events_in_order() {
        let _serial = crate::slight::gimmicks::TEST_LOCK.lock().unwrap();
        EVENTS.lock().clear();
        for port in 0..(MAX_QUEUED_EVENTS as u16 + 2) {
            emit(GameEvent::Point { host: 9, port });
        }
        let events = drain();
        assert_eq!(events.len(), MAX_QUEUED_EVENTS);
        assert!(matches!(
            events.first(),
            Some(GameEvent::Point { host: 9, port: 2 })
        ));
        assert!(
            matches!(events.last(), Some(GameEvent::Point { host: 9, port }) if *port == MAX_QUEUED_EVENTS as u16 + 1)
        );
    }
}
