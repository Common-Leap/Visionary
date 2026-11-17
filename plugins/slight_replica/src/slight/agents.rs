//! Agent registry — global fighters / founder linkage (DAT_71001e1b80 layout).

use std::collections::{BTreeMap, HashMap};

use parking_lot::Mutex;
use smash::app::lua_bind::{StatusModule, WorkModule};
use smash::app::sv_battle_object;
use smash::lib::lua_const::{
    FIGHTER_INSTANCE_WORK_ID_INT_DEAD_REASON, FIGHTER_INSTANCE_WORK_ID_INT_KO_ENTRY_ID,
    FIGHTER_STATUS_KIND_DEAD, FIGHTER_STATUS_KIND_REBIRTH,
};
use std::sync::LazyLock;

use super::effect_viewer::effect_data::EffectData;

pub const MAX_BOID: u32 = 512;
pub const BOID_MASK: u32 = 0x1FF;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRecord {
    pub boid: u32,
    pub module_accessor_addr: u64,
    pub category: i32,
    pub kind: i32,
    pub status_kind: i32,
    pub entry_id: i32,
    pub founder_entry_id: Option<i32>,
    /// Sampled only while the exact shared DEAD status is live. Keeping it in
    /// the same probe closes the race where reset/rebirth clears these work
    /// values before the event facade runs.
    dead_metadata: Option<DeadMetadata>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DeadMetadata {
    reason: i32,
    ko_entry_id: i32,
}

/// A real fighter status edge sampled from the live agent registry.
///
/// The producer never invents an old status: consumers receive both exact
/// `StatusModule::status_kind` values plus the complete identity and stock
/// generation captured by the same lifecycle comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusTransition {
    pub boid: u32,
    pub module_accessor_addr: u64,
    pub fighter_kind: i32,
    pub entry_id: i32,
    pub previous_status: i32,
    pub status: i32,
    pub stock_generation: u32,
}

/// Lost-stock observation sampled on the exact transition into
/// `FIGHTER_STATUS_KIND_DEAD`. `onKO` consumes the cause-agnostic edge, while
/// the richer reason metadata remains available for diagnostics and any future
/// cause-specific hooks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FighterDownObservation {
    pub boid: u32,
    pub fighter_kind: i32,
    pub entry_id: i32,
    pub dead_reason: i32,
    pub ko_entry_id: i32,
    pub stock_generation: u32,
}

/// Exact transition into the shared fighter REBIRTH status. Unlike the
/// legacy reinit facade, this observation comes from the same identity-aware
/// status tracker that owns stock generations, so it cannot confuse an
/// ordinary motion refresh with a new stock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FighterRebirthObservation {
    pub boid: u32,
    pub fighter_kind: i32,
    pub entry_id: i32,
    pub stock_generation: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FrameObservation {
    status: Option<StatusTransition>,
    fighter_down: Option<FighterDownObservation>,
    fighter_rebirth: Option<FighterRebirthObservation>,
}

/// Identity plus the last status safe to compare for one fighter lifecycle.
/// A boid is not a lifetime token: the game may recycle it.  The module
/// accessor, fighter kind, and player entry therefore have to agree before a
/// changed status can be called an edge.  Explicit reset handling erases this
/// baseline too, because a reset status is lifecycle plumbing, not a move
/// transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StatusObservation {
    module_accessor_addr: u64,
    kind: i32,
    entry_id: i32,
    status: i32,
    stock_generation: u32,
    down_emitted: bool,
    dead_metadata: Option<DeadMetadata>,
}

impl StatusObservation {
    fn from_record(record: &AgentRecord) -> Option<Self> {
        // `StatusModule::status_kind` is a signed fighter-only value.  Do
        // not establish an edge baseline from a transient/unknown object
        // record: a stale category, kind, or entry id must fail closed rather
        // than borrowing another fighter's lifecycle at a recycled boid.
        (record.category == 0
            && record.module_accessor_addr != 0
            && record.kind >= 0
            && record.entry_id >= 0
            && record.status_kind >= 0)
            .then_some(Self {
                module_accessor_addr: record.module_accessor_addr,
                kind: record.kind,
                entry_id: record.entry_id,
                status: record.status_kind,
                stock_generation: 0,
                // Discovering an object already in DEAD cannot prove the
                // preceding live -> dead edge. Mark it consumed rather than
                // fabricating a lost-stock observation.
                down_emitted: record.status_kind == *FIGHTER_STATUS_KIND_DEAD,
                dead_metadata: record.dead_metadata,
            })
    }

    fn same_lifecycle(self, other: Self) -> bool {
        self.module_accessor_addr == other.module_accessor_addr
            && self.kind == other.kind
            && self.entry_id == other.entry_id
    }

    fn identity(self) -> (u64, i32, i32) {
        (self.module_accessor_addr, self.kind, self.entry_id)
    }
}

/// Edge detector independent of the registry's mutable snapshots.  `upsert`
/// refreshes records during each agent callback, so using `AgentRecord` alone
/// would overwrite the old status before the once-per-frame pass can compare
/// it.  This separate baseline makes the frame pass the single producer and
/// ensures duplicate snapshots do not produce duplicate transitions.
#[derive(Default)]
struct StatusEdgeTracker {
    observed: BTreeMap<u32, StatusObservation>,
    /// Survives a stock reset for the same concrete fighter identity. It is
    /// cleared with the match registry, and a recycled accessor/kind/entry
    /// tuple starts at zero instead of inheriting another object's stock.
    generations: BTreeMap<(u64, i32, i32), u32>,
}

impl StatusEdgeTracker {
    /// A newly discovered object establishes a baseline only.  Existing
    /// lifecycles retain their last frame baseline even if `upsert_module`
    /// refreshes the registry record between global frame passes.
    fn baseline_new_lifecycle(&mut self, record: &AgentRecord) {
        let Some(mut next) = StatusObservation::from_record(record) else {
            self.forget(record.boid);
            return;
        };
        next.stock_generation = *self.generations.entry(next.identity()).or_insert(0);
        match self.observed.get(&record.boid).copied() {
            Some(previous) if previous.same_lifecycle(next) => {}
            _ => {
                self.observed.insert(record.boid, next);
            }
        }
    }

    /// Compare one frame snapshot against its established lifecycle baseline.
    /// First observation and a recycled object are deliberately baselines,
    /// never edges.  The new status becomes the baseline before delivery, so
    /// repeated snapshots can never enqueue the same edge twice.
    fn observe_frame(&mut self, record: &AgentRecord) -> FrameObservation {
        let Some(mut next) = StatusObservation::from_record(record) else {
            // Negative status values are lifecycle sentinels in the engine.
            // Removing, rather than retaining, the prior baseline is
            // important: if the reset callback is delayed or skipped, the
            // next valid status still becomes a first observation and cannot
            // turn a death/respawn into a move edge.
            self.forget(record.boid);
            return FrameObservation::default();
        };
        next.stock_generation = *self.generations.entry(next.identity()).or_insert(0);
        let previous = self.observed.get(&record.boid).copied();
        let mut observed = FrameObservation::default();
        if let Some(previous) = previous.filter(|previous| previous.same_lifecycle(next)) {
            next.stock_generation = previous.stock_generation;
            next.down_emitted = previous.down_emitted;

            if previous.status != next.status {
                // `from_record` has already rejected signed lifecycle
                // sentinels, so this is an exact fighter status edge.
                observed.status = Some(StatusTransition {
                    boid: record.boid,
                    module_accessor_addr: next.module_accessor_addr,
                    fighter_kind: record.kind,
                    entry_id: record.entry_id,
                    previous_status: previous.status,
                    status: next.status,
                    stock_generation: next.stock_generation,
                });
            }

            if next.status == *FIGHTER_STATUS_KIND_REBIRTH
                && previous.status != *FIGHTER_STATUS_KIND_REBIRTH
            {
                next.stock_generation = previous.stock_generation.saturating_add(1);
                next.down_emitted = false;
                self.generations
                    .insert(next.identity(), next.stock_generation);
                observed.fighter_rebirth = Some(FighterRebirthObservation {
                    boid: record.boid,
                    fighter_kind: record.kind,
                    entry_id: record.entry_id,
                    stock_generation: next.stock_generation,
                });
            }

            if next.status == *FIGHTER_STATUS_KIND_DEAD
                && !previous.down_emitted
                && (previous.status != *FIGHTER_STATUS_KIND_DEAD
                    || previous.dead_metadata.is_none())
            {
                if let Some(metadata) = next.dead_metadata {
                    observed.fighter_down = Some(FighterDownObservation {
                        boid: record.boid,
                        fighter_kind: record.kind,
                        entry_id: record.entry_id,
                        dead_reason: metadata.reason,
                        ko_entry_id: metadata.ko_entry_id,
                        stock_generation: next.stock_generation,
                    });
                    next.down_emitted = true;
                }
            }
        }
        self.observed.insert(record.boid, next);
        observed
    }

    fn forget(&mut self, boid: u32) {
        if let Some(previous) = self.observed.remove(&boid) {
            if previous.down_emitted {
                self.generations.insert(
                    previous.identity(),
                    previous.stock_generation.saturating_add(1),
                );
            }
        }
    }

    fn clear(&mut self) {
        self.observed.clear();
        self.generations.clear();
    }
}

struct AgentRegistry {
    by_boid: HashMap<u32, AgentRecord>,
    registration_order: HashMap<u32, u64>,
    next_registration: u64,
    status_edges: StatusEdgeTracker,
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self {
            by_boid: HashMap::with_capacity(64),
            registration_order: HashMap::with_capacity(64),
            next_registration: 0,
            status_edges: StatusEdgeTracker::default(),
        }
    }
}

impl AgentRegistry {
    /// Battle object ids are FULL ids (e.g. 0x2000000x for fighters) — they cannot be
    /// enumerated by scanning 0..512, so refresh only PRUNES dead entries and re-probes the
    /// live ones (status changes each frame). New agents enter via `upsert_module` (per-agent
    /// smashline callbacks + effect-spawn hooks).
    fn refresh(&mut self) -> Vec<FrameObservation> {
        // HashMap key order is intentionally randomized.  Sorting keeps
        // status event order stable across equal snapshots and lets every
        // later consumer observe a deterministic fighter sequence.
        let mut ids: Vec<u32> = self.by_boid.keys().copied().collect();
        ids.sort_unstable();
        let mut transitions = Vec::new();
        for boid in ids {
            match probe(boid) {
                Some(rec) => {
                    self.observe_registration(&rec);
                    self.by_boid.insert(boid, rec.clone());
                    let observation = self.status_edges.observe_frame(&rec);
                    if observation != FrameObservation::default() {
                        transitions.push(observation);
                    }
                }
                None => {
                    self.by_boid.remove(&boid);
                    self.registration_order.remove(&boid);
                    self.status_edges.forget(boid);
                }
            }
        }
        transitions
    }

    fn upsert_module(
        &mut self,
        module_accessor: *mut smash::app::BattleObjectModuleAccessor,
    ) -> Option<AgentRecord> {
        let boid = boid_from_module(module_accessor)?;
        let rec = probe(boid)?;
        self.status_edges.baseline_new_lifecycle(&rec);
        self.observe_registration(&rec);
        self.by_boid.insert(boid, rec.clone());
        Some(rec)
    }

    fn observe_registration(&mut self, record: &AgentRecord) {
        if self.by_boid.get(&record.boid).is_none_or(|old| {
            old.module_accessor_addr != record.module_accessor_addr
                || old.kind != record.kind
                || old.category != record.category
        }) {
            self.next_registration += 1;
            self.registration_order
                .insert(record.boid, self.next_registration);
        }
    }

    fn newest_registered(&self, candidates: &[u32]) -> Option<u32> {
        candidates
            .iter()
            .filter_map(|boid| {
                self.registration_order
                    .get(boid)
                    .map(|order| (*order, *boid))
            })
            .max()
            .map(|(_, boid)| boid)
    }

    fn get(&self, boid: u32) -> Option<&AgentRecord> {
        self.by_boid.get(&boid)
    }

    fn clear(&mut self) {
        self.by_boid.clear();
        self.registration_order.clear();
        self.next_registration = 0;
        self.status_edges.clear();
    }

    fn forget_status_observation(&mut self, boid: u32) {
        self.status_edges.forget(boid);
    }

    fn live_accessors(&self) -> Vec<(u64, *mut smash::app::BattleObjectModuleAccessor)> {
        let mut out = Vec::new();
        for boid in self.by_boid.keys() {
            if !is_live(*boid) {
                continue;
            }
            let ptr = unsafe { sv_battle_object::module_accessor(*boid) };
            if !ptr.is_null() {
                out.push((ptr as u64, ptr));
            }
        }
        out
    }
}

static AGENTS: LazyLock<Mutex<AgentRegistry>> =
    LazyLock::new(|| Mutex::new(AgentRegistry::default()));

pub fn refresh_all() {
    // Status emission and lifecycle reset use the one lock order
    // `AGENTS -> EVENTS`. `emit` only appends to the queue; it does not
    // dispatch consumers, so this cannot call back into AGENTS. Holding both
    // operations together closes the otherwise-racy window where a reset
    // could discard an edge before this producer appended it.
    let mut agents = AGENTS.lock();
    for observation in agents.refresh() {
        if let Some(transition) = observation.status {
            crate::slight::systems::event_system::emit(
                crate::slight::systems::event_system::GameEvent::StatusChange {
                    boid: transition.boid,
                    module_accessor_addr: transition.module_accessor_addr,
                    fighter_kind: transition.fighter_kind,
                    entry_id: transition.entry_id,
                    previous_status: transition.previous_status,
                    status: transition.status,
                    stock_generation: transition.stock_generation,
                },
            );
        }
        if let Some(down) = observation.fighter_down {
            crate::slight::systems::event_system::emit(
                crate::slight::systems::event_system::GameEvent::FighterDownObserved {
                    boid: down.boid,
                    fighter_kind: down.fighter_kind,
                    entry_id: down.entry_id,
                    dead_reason: down.dead_reason,
                    ko_entry_id: down.ko_entry_id,
                    stock_generation: down.stock_generation,
                },
            );
        }
        if let Some(rebirth) = observation.fighter_rebirth {
            crate::slight::systems::event_system::emit(
                crate::slight::systems::event_system::GameEvent::FighterRebirthObserved {
                    boid: rebirth.boid,
                    fighter_kind: rebirth.fighter_kind,
                    entry_id: rebirth.entry_id,
                    stock_generation: rebirth.stock_generation,
                },
            );
        }
    }
}

pub fn upsert_module(
    module_accessor: *mut smash::app::BattleObjectModuleAccessor,
) -> Option<AgentRecord> {
    AGENTS.lock().upsert_module(module_accessor)
}

pub fn lookup(boid: u32) -> Option<AgentRecord> {
    AGENTS.lock().get(boid).cloned()
}

/// Read the generation captured by the status sampler only when this exact registry lifetime
/// still owns the observation. Diagnostic readers must not borrow a recycled object's stock.
pub fn stock_generation(record: &AgentRecord) -> Option<u32> {
    let registry = AGENTS.lock();
    let observed = registry.status_edges.observed.get(&record.boid)?;
    (observed.module_accessor_addr == record.module_accessor_addr
        && observed.kind == record.kind
        && observed.entry_id == record.entry_id)
        .then_some(observed.stock_generation)
}

/// Validate that a queued status edge still belongs to the exact live native
/// fighter identity that produced it. BOID, kind, entry, and stock generation
/// alone are insufficient when the engine recycles a BOID with the same
/// fighter slot; the captured module accessor closes that gap.
pub fn status_transition_is_current(
    boid: u32,
    module_accessor_addr: u64,
    fighter_kind: i32,
    entry_id: i32,
    expected_generation: u32,
) -> bool {
    let Some(record) = lookup(boid) else {
        return false;
    };
    if !status_identity_matches(
        &record,
        module_accessor_addr,
        fighter_kind,
        entry_id,
        expected_generation,
        stock_generation(&record),
    ) {
        return false;
    }
    unsafe {
        if !sv_battle_object::is_active(boid) || sv_battle_object::category(boid) != 0 {
            return false;
        }
        let accessor = sv_battle_object::module_accessor(boid);
        !accessor.is_null()
            && accessor as u64 == module_accessor_addr
            && sv_battle_object::kind(boid) == fighter_kind
            && sv_battle_object::entry_id(boid) == entry_id
    }
}

fn status_identity_matches(
    record: &AgentRecord,
    module_accessor_addr: u64,
    fighter_kind: i32,
    entry_id: i32,
    expected_generation: u32,
    observed_generation: Option<u32>,
) -> bool {
    record.category == 0
        && record.kind == fighter_kind
        && record.entry_id == entry_id
        && record.module_accessor_addr == module_accessor_addr
        && observed_generation == Some(expected_generation)
}

/// Return whether `boid` is the one fighter callback that drives the global frame pass.
///
/// `StatusLine::Main` is per agent, not a frame clock.  Treating the first repeated battle
/// object id as a frame edge was fragile: an agent may re-enter its Main line before every
/// weapon has run, especially in busy CPU matches, and each false edge re-ran the complete
/// global fighter and weapon dispatch.  Use the lowest live fighter entry instead.  Smash
/// assigns stable non-negative entry ids for the entire match, so exactly that fighter's Main
/// callback is a reliable once-per-game-frame driver.  The BOID tie-breaker keeps malformed or
/// transient duplicate entries deterministic.
pub fn is_frame_driver(boid: u32) -> bool {
    let registry = AGENTS.lock();
    frame_driver_boid(registry.by_boid.values(), is_live).is_some_and(|driver| driver == boid)
}

/// Select the frame driver from live, valid fighter records only. Keeping the
/// selection pure lets the global pass recover when an old lower-entry record
/// remains in the registry until the next prune; otherwise that stale record
/// could suppress every live fighter's once-per-frame work.
fn frame_driver_boid<'a>(
    records: impl Iterator<Item = &'a AgentRecord>,
    is_live: impl Fn(u32) -> bool,
) -> Option<u32> {
    records
        .filter(|rec| rec.category == 0 && rec.kind >= 0 && rec.entry_id >= 0 && is_live(rec.boid))
        .min_by_key(|rec| (rec.entry_id, rec.boid))
        .map(|rec| rec.boid)
}

/// Jorge FUN_71000db2b4 — lookup live fighter by founder entry id.
pub fn lookup_by_founder(founder_id: i32) -> Option<AgentRecord> {
    if founder_id < 0 {
        return None;
    }
    AGENTS
        .lock()
        .by_boid
        .values()
        .find(|rec| {
            rec.category == 0
                && (rec.entry_id == founder_id || rec.founder_entry_id == Some(founder_id))
        })
        .cloned()
}

pub fn has_initialized(boid: u32) -> bool {
    crate::slight::systems::dynamic_memory::has_agent(boid)
}

/// Newest observed object lifetime among an already validated set of live objects.
pub fn newest_registered(candidates: &[u32]) -> Option<u32> {
    AGENTS.lock().newest_registered(candidates)
}

pub fn all_records() -> Vec<AgentRecord> {
    AGENTS.lock().by_boid.values().cloned().collect()
}

pub fn clear_all() {
    AGENTS.lock().clear();
}

/// Reset the status-edge baseline for this object lifecycle.  The reset path
/// also drops already queued status edges so a death/respawn cannot turn into
/// a synthetic powered-move event when the event facade runs later that
/// frame.
pub fn reset_status_observation(boid: u32) {
    // Keep the same `AGENTS -> EVENTS` order as `refresh_all`. In addition to
    // avoiding lock inversion, this makes reset and queued-edge removal one
    // lifecycle boundary: a producer cannot append the old edge after this
    // reset has completed.
    let mut agents = AGENTS.lock();
    agents.forget_status_observation(boid);
    crate::slight::systems::event_system::discard_status_changes(boid);
}

pub fn live_accessors() -> Vec<(u64, *mut smash::app::BattleObjectModuleAccessor)> {
    AGENTS.lock().live_accessors()
}

pub fn boid_from_module(
    module_accessor: *mut smash::app::BattleObjectModuleAccessor,
) -> Option<u32> {
    if module_accessor.is_null() {
        return None;
    }
    unsafe {
        // FULL battle object id — sv_battle_object::* take the full id, NOT a 9-bit index.
        // (Masking with BOID_MASK here made the identity check below fail for all but one
        // fighter, so the registry only ever saw 1 agent and everyone else's effects were
        // treated as owned by dead accessors.)
        let boid = (*module_accessor).battle_object_id;
        if !is_live(boid) {
            return None;
        }
        if sv_battle_object::module_accessor(boid) != module_accessor {
            return None;
        }
        Some(boid)
    }
}

fn is_live(boid: u32) -> bool {
    unsafe { !sv_battle_object::is_null(boid) && sv_battle_object::is_active(boid) }
}

fn probe(boid: u32) -> Option<AgentRecord> {
    if !is_live(boid) {
        return None;
    }
    unsafe {
        let ptr = sv_battle_object::module_accessor(boid);
        if ptr.is_null() {
            return None;
        }
        let category = sv_battle_object::category(boid);
        let kind = sv_battle_object::kind(boid);
        let entry_id = sv_battle_object::entry_id(boid);
        let status_kind = if category == 0 {
            StatusModule::status_kind(ptr)
        } else {
            0
        };
        let dead_metadata =
            (category == 0 && status_kind == *FIGHTER_STATUS_KIND_DEAD).then(|| DeadMetadata {
                reason: WorkModule::get_int(ptr, *FIGHTER_INSTANCE_WORK_ID_INT_DEAD_REASON),
                ko_entry_id: WorkModule::get_int(ptr, *FIGHTER_INSTANCE_WORK_ID_INT_KO_ENTRY_ID),
            });
        let founder_entry_id = if category == 0 {
            None
        } else {
            let raw = sv_battle_object::get_founder_id(boid);
            if raw >= 0 && is_live(raw as u32) {
                let fe = sv_battle_object::entry_id(raw as u32);
                (fe >= 0).then_some(fe)
            } else {
                None
            }
        };
        Some(AgentRecord {
            boid,
            module_accessor_addr: ptr as u64,
            category,
            kind,
            status_kind,
            entry_id,
            founder_entry_id,
            dead_metadata,
        })
    }
}

pub fn format_effect_name(
    id: u64,
    category: i32,
    object_kind: i32,
    entry_id: i32,
    founder_entry_id: Option<i32>,
    data: &EffectData,
) -> String {
    // The original has no "Effect #..." fallback string in its binary; the effect name is
    // always the resolved hash label (e.g. "0x0" for a zero hash), used as-is.
    let _ = id;
    let label = data.effect_name.clone();
    // The ignored reference binary's rodata contains exactly one effect-name
    // template: "<name> of weapon with kind <kind> (player: <player>)" with the unknown-owner
    // fallback spelled "Unknwon fighter" — Jorge's original typo, reproduced here for parity.
    // There is no "of agent kind" or "(fighter kind ...)" template in the binary, so any
    // non-fighter object uses the weapon template and a plain fighter uses just its name.
    match category {
        0 => label,
        _ => {
            let player = founder_entry_id
                .or((entry_id >= 0).then_some(entry_id))
                .map(|e| format!("entry {e}"))
                .unwrap_or_else(|| "Unknwon fighter".into());
            format!("{label} of weapon with kind {object_kind} (player: {player})")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fighter(boid: u32, status: i32) -> AgentRecord {
        AgentRecord {
            boid,
            module_accessor_addr: 0x1000 + boid as u64,
            category: 0,
            kind: 0x1234,
            status_kind: status,
            entry_id: 1,
            founder_entry_id: None,
            dead_metadata: None,
        }
    }

    #[test]
    fn newest_registration_tracks_lifetimes_instead_of_object_ids_or_refresh_order() {
        let mut registry = AgentRegistry::default();
        let mut first = fighter(20, 0);
        let second = fighter(3, 0);
        for record in [&first, &second] {
            registry.observe_registration(record);
            registry.by_boid.insert(record.boid, record.clone());
        }
        assert_eq!(registry.newest_registered(&[20, 3]), Some(3));
        registry.observe_registration(&first);
        assert_eq!(registry.newest_registered(&[3, 20]), Some(3));
        first.module_accessor_addr += 1;
        registry.observe_registration(&first);
        registry.by_boid.insert(first.boid, first);
        assert_eq!(registry.newest_registered(&[20, 3]), Some(20));
        assert_eq!(registry.newest_registered(&[3]), Some(3));
        assert_eq!(registry.newest_registered(&[]), None);
        registry.clear();
        assert_eq!(registry.newest_registered(&[20, 3]), None);
    }

    fn observe_status(
        tracker: &mut StatusEdgeTracker,
        record: &AgentRecord,
    ) -> Option<StatusTransition> {
        tracker.observe_frame(record).status
    }

    fn transition(record: &AgentRecord, previous_status: i32) -> StatusTransition {
        StatusTransition {
            boid: record.boid,
            module_accessor_addr: record.module_accessor_addr,
            fighter_kind: record.kind,
            entry_id: record.entry_id,
            previous_status,
            status: record.status_kind,
            stock_generation: 0,
        }
    }

    fn dead_fighter(boid: u32, reason: i32, ko_entry_id: i32) -> AgentRecord {
        let mut record = fighter(boid, *FIGHTER_STATUS_KIND_DEAD);
        record.dead_metadata = Some(DeadMetadata {
            reason,
            ko_entry_id,
        });
        record
    }

    #[test]
    fn status_edges_are_real_exact_and_deduplicated() {
        let mut tracker = StatusEdgeTracker::default();
        let initial = fighter(0x2000_0001, 10);
        // Both first-observation routes establish a baseline only.
        tracker.baseline_new_lifecycle(&initial);
        assert_eq!(observe_status(&mut tracker, &initial), None);

        let changed = fighter(initial.boid, 11);
        assert_eq!(
            observe_status(&mut tracker, &changed),
            Some(transition(&changed, 10))
        );
        // Repeating either the status or its snapshot cannot fire it twice.
        assert_eq!(observe_status(&mut tracker, &changed), None);
    }

    #[test]
    fn status_edges_do_not_cross_reset_or_recycled_lifecycles() {
        let mut tracker = StatusEdgeTracker::default();
        let initial = fighter(0x2000_0002, 20);
        tracker.baseline_new_lifecycle(&initial);

        // A reset makes the following observation a new baseline, even when
        // a stock retains the same battle object id.
        tracker.forget(initial.boid);
        let after_reset = fighter(initial.boid, 21);
        assert_eq!(observe_status(&mut tracker, &after_reset), None);

        let next = fighter(initial.boid, 22);
        assert_eq!(
            observe_status(&mut tracker, &next),
            Some(transition(&next, 21))
        );

        // A recycled boid with a new accessor (or kind/entry) is likewise a
        // first observation.  Only its later in-lifecycle status can fire.
        let mut recycled = fighter(initial.boid, 30);
        recycled.module_accessor_addr += 0x100;
        recycled.kind += 1;
        recycled.entry_id += 1;
        assert_eq!(observe_status(&mut tracker, &recycled), None);
        recycled.status_kind = 31;
        assert_eq!(
            observe_status(&mut tracker, &recycled),
            Some(transition(&recycled, 30))
        );
    }

    #[test]
    fn queued_status_identity_rejects_same_slot_recycle_and_accepts_fresh_edge() {
        let old_identity = fighter(0x2000_0003, 30);
        let mut recycled = old_identity.clone();
        recycled.module_accessor_addr = old_identity.module_accessor_addr + 0x100;

        // BOID, kind, player entry, and stock generation can all be reused.
        // The captured accessor is what distinguishes this stale queued edge.
        assert!(!status_identity_matches(
            &recycled,
            old_identity.module_accessor_addr,
            old_identity.kind,
            old_identity.entry_id,
            0,
            Some(0),
        ));
        assert!(status_identity_matches(
            &recycled,
            recycled.module_accessor_addr,
            recycled.kind,
            recycled.entry_id,
            0,
            Some(0),
        ));

        let mut tracker = StatusEdgeTracker::default();
        tracker.baseline_new_lifecycle(&recycled);
        assert_eq!(observe_status(&mut tracker, &recycled), None);
        let changed = AgentRecord {
            status_kind: 31,
            ..recycled.clone()
        };
        let edge = observe_status(&mut tracker, &changed).expect("fresh identity edge");
        assert_eq!(edge.module_accessor_addr, recycled.module_accessor_addr);
        assert_eq!(edge.previous_status, 30);
        assert_eq!(edge.status, 31);
    }

    #[test]
    fn dead_and_nonfighter_snapshots_are_never_status_events() {
        let mut tracker = StatusEdgeTracker::default();
        let initial = fighter(0x2000_0003, 40);
        tracker.baseline_new_lifecycle(&initial);
        let dead = fighter(initial.boid, -1);
        assert_eq!(observe_status(&mut tracker, &dead), None);

        // A delayed/absent reset callback cannot promote respawn into an
        // edge: the negative sentinel removed the old baseline above.
        assert_eq!(
            observe_status(&mut tracker, &fighter(initial.boid, 41)),
            None
        );

        let mut weapon = fighter(initial.boid, 99);
        weapon.category = 1;
        assert_eq!(observe_status(&mut tracker, &weapon), None);
        // A later fighter snapshot is new after the nonfighter removal.
        assert_eq!(
            observe_status(&mut tracker, &fighter(initial.boid, 50)),
            None
        );
    }

    #[test]
    fn status_edges_reject_invalid_identity_and_preserve_signed_status_width() {
        let mut tracker = StatusEdgeTracker::default();
        let initial = fighter(0x2000_0004, i32::MAX - 1);
        tracker.baseline_new_lifecycle(&initial);
        let max = fighter(initial.boid, i32::MAX);
        assert_eq!(
            observe_status(&mut tracker, &max),
            Some(transition(&max, i32::MAX - 1))
        );

        for mut invalid in [fighter(0x2000_0005, 1), fighter(0x2000_0006, 1)] {
            if invalid.boid == 0x2000_0005 {
                invalid.kind = -1;
            } else {
                invalid.entry_id = -1;
            }
            tracker.baseline_new_lifecycle(&invalid);
            assert_eq!(observe_status(&mut tracker, &invalid), None);
        }
    }

    #[test]
    fn fighter_down_is_complete_exactly_once_per_observed_stock() {
        let mut tracker = StatusEdgeTracker::default();
        let initial = fighter(0x2000_0007, 10);
        tracker.baseline_new_lifecycle(&initial);

        let dead = dead_fighter(initial.boid, 4, 2);
        let first = tracker.observe_frame(&dead);
        assert_eq!(first.status.unwrap().status, *FIGHTER_STATUS_KIND_DEAD);
        assert_eq!(
            first.fighter_down,
            Some(FighterDownObservation {
                boid: initial.boid,
                fighter_kind: initial.kind,
                entry_id: initial.entry_id,
                dead_reason: 4,
                ko_entry_id: 2,
                stock_generation: 0,
            })
        );
        assert_eq!(
            tracker.observe_frame(&dead).fighter_down,
            None,
            "repeated DEAD frames must not duplicate a stock loss"
        );

        // Leaving DEAD without the reviewed REBIRTH boundary does not arm a
        // second event for the same stock.
        tracker.observe_frame(&fighter(initial.boid, 11));
        assert_eq!(tracker.observe_frame(&dead).fighter_down, None);

        let rebirth = fighter(initial.boid, *FIGHTER_STATUS_KIND_REBIRTH);
        let rebirth_observation = tracker.observe_frame(&rebirth);
        assert_eq!(
            rebirth_observation.fighter_rebirth,
            Some(FighterRebirthObservation {
                boid: initial.boid,
                fighter_kind: initial.kind,
                entry_id: initial.entry_id,
                stock_generation: 1,
            })
        );
        assert_eq!(
            tracker.observe_frame(&rebirth).fighter_rebirth,
            None,
            "remaining in REBIRTH must not duplicate stock-start"
        );
        tracker.observe_frame(&fighter(initial.boid, 12));
        let second = tracker.observe_frame(&dead).fighter_down.unwrap();
        assert_eq!(second.stock_generation, 1);

        // A signed/reset lifecycle may clear the status baseline before the
        // next REBIRTH edge is observable. Forgetting an already emitted
        // stock retains and increments its identity-scoped generation.
        tracker.forget(initial.boid);
        let next_stock = fighter(initial.boid, 13);
        assert_eq!(
            tracker.observe_frame(&next_stock),
            FrameObservation::default()
        );
        assert_eq!(
            tracker
                .observe_frame(&dead)
                .fighter_down
                .unwrap()
                .stock_generation,
            2
        );
    }

    #[test]
    fn fighter_down_fails_closed_without_edge_identity_or_cause_data() {
        let mut tracker = StatusEdgeTracker::default();
        let initial = fighter(0x2000_0008, 10);
        tracker.baseline_new_lifecycle(&initial);

        // If the cause fields are not available on the edge frame, retain the
        // pending observation and accept them only while the same identity is
        // still in DEAD.
        let dead_without_cause = fighter(initial.boid, *FIGHTER_STATUS_KIND_DEAD);
        assert_eq!(
            tracker.observe_frame(&dead_without_cause).fighter_down,
            None
        );
        let complete = dead_fighter(initial.boid, 7, -1);
        assert_eq!(
            tracker
                .observe_frame(&complete)
                .fighter_down
                .unwrap()
                .dead_reason,
            7
        );

        // First discovering an object after it is already dead cannot prove
        // the transition and therefore never emits.
        let mut late = StatusEdgeTracker::default();
        late.baseline_new_lifecycle(&complete);
        assert_eq!(late.observe_frame(&complete).fighter_down, None);

        // A recycled identity is another baseline, not a continuation.
        let mut recycled = complete.clone();
        recycled.module_accessor_addr += 0x100;
        assert_eq!(tracker.observe_frame(&recycled).fighter_down, None);
    }

    #[test]
    fn stale_or_invalid_records_cannot_steal_the_frame_driver() {
        let stale = fighter(0x2000_0010, 1);
        let live = fighter(0x2000_0011, 1);
        let mut invalid = fighter(0x2000_0012, 1);
        invalid.kind = -1;
        assert_eq!(
            frame_driver_boid([&stale, &live, &invalid].into_iter(), |boid| boid
                == live.boid),
            Some(live.boid)
        );
    }
}
