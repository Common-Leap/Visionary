//! Gimmick variable state (slice F2.4).
//!
//! Per-fighter-object storage for behavior variables: floats, ints, and
//! flags keyed by object id and variable name. The generated F2.3 state
//! structs are per-fighter types; this store is the typeless floor beneath
//! them — teardown wipes entries, so a new stock can never inherit a dead
//! object's gauge. Pure logic, no engine calls: safe to unit test.

use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// One variable value. Mirrors the F2.1 [`VarType`](crate::gimmicks::VarType)
/// set minus timers: countdowns live in `timer_system`, not here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VarValue {
    Float(f32),
    Int(i32),
    Flag(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActorIdentity {
    pub fighter_kind: i32,
    pub entry_id: i32,
    pub module_accessor_addr: u64,
    pub stock_generation: u32,
}

#[derive(Default)]
struct ActorState {
    identity: Option<ActorIdentity>,
    vars: BTreeMap<String, VarValue>,
}

#[derive(Default)]
struct Store {
    actors: BTreeMap<u32, ActorState>,
    pending: BTreeMap<u32, u64>,
    next_ticket: u64,
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));

/// A bounded optimistic transaction for one static event. The actor row and
/// token are installed atomically; lifecycle/reset writes invalidate the
/// token under the same lock used by commit.
pub(crate) struct Transaction {
    boid: u32,
    ticket: u64,
    identity: ActorIdentity,
    values: BTreeMap<String, VarValue>,
}

impl Transaction {
    pub(crate) fn get(&self, name: &str) -> Option<VarValue> {
        self.values.get(name).copied()
    }

    pub(crate) fn commit(mut self, writes: &BTreeMap<String, VarValue>) -> bool {
        let mut store = STORE.lock();
        if store.pending.get(&self.boid) != Some(&self.ticket)
            || !store
                .actors
                .get(&self.boid)
                .is_some_and(|actor| actor.identity == Some(self.identity))
        {
            self.cancel_locked(&mut store);
            return false;
        }
        let actor = store
            .actors
            .get_mut(&self.boid)
            .expect("validated actor row");
        actor
            .vars
            .extend(writes.iter().map(|(key, value)| (key.clone(), *value)));
        store.pending.remove(&self.boid);
        self.ticket = 0;
        prune_actor(&mut store, self.boid);
        true
    }

    pub(crate) fn is_current(&self) -> bool {
        let store = STORE.lock();
        store.pending.get(&self.boid) == Some(&self.ticket)
            && store
                .actors
                .get(&self.boid)
                .is_some_and(|actor| actor.identity == Some(self.identity))
    }

    fn cancel_locked(&mut self, store: &mut Store) {
        if self.ticket != 0 && store.pending.get(&self.boid) == Some(&self.ticket) {
            store.pending.remove(&self.boid);
        }
        self.ticket = 0;
        prune_actor(store, self.boid);
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if self.ticket == 0 {
            return;
        }
        let mut store = STORE.lock();
        self.cancel_locked(&mut store);
    }
}

fn prune_actor(store: &mut Store, boid: u32) {
    if store.pending.contains_key(&boid) {
        return;
    }
    if store
        .actors
        .get(&boid)
        .is_some_and(|actor| actor.vars.is_empty())
    {
        store.actors.remove(&boid);
    }
}

pub(crate) fn begin(boid: u32, identity: ActorIdentity) -> Transaction {
    let mut store = STORE.lock();
    let actor = store.actors.entry(boid).or_default();
    // Direct diagnostic/state writes have no identity attached yet. Preserve
    // those values for the first exact runtime identity that claims the row;
    // once an identity has been observed, a different identity always starts
    // from defaults even if no lifecycle callback ran for the recycled BOID.
    if actor.identity.is_some_and(|previous| previous != identity) {
        actor.vars.clear();
    }
    actor.identity = Some(identity);
    store.next_ticket = store.next_ticket.wrapping_add(1).max(1);
    let ticket = store.next_ticket;
    store.pending.insert(boid, ticket);
    let values = store
        .actors
        .get(&boid)
        .expect("actor row was just installed")
        .vars
        .clone();
    Transaction {
        boid,
        ticket,
        identity,
        values,
    }
}

/// Write one variable. Creates the object's entry on first write.
pub fn set(boid: u32, name: &str, value: VarValue) {
    let mut store = STORE.lock();
    store.pending.remove(&boid);
    store
        .actors
        .entry(boid)
        .or_default()
        .vars
        .insert(name.to_string(), value);
}

/// Read one variable. `None` is an unset variable, never a zero: handlers
/// must distinguish "never charged" from "charged to zero".
pub fn get(boid: u32, name: &str) -> Option<VarValue> {
    STORE
        .lock()
        .actors
        .get(&boid)
        .and_then(|actor| actor.vars.get(name).copied())
}

pub(crate) fn get_for_identity(boid: u32, identity: ActorIdentity, name: &str) -> Option<VarValue> {
    let store = STORE.lock();
    store
        .actors
        .get(&boid)
        .filter(|actor| actor.identity == Some(identity))?
        .vars
        .get(name)
        .copied()
}

/// Drop one object's whole entry: stock loss, object removal, match exit.
/// After this, every read for the object is `None`.
pub fn reset_boid(boid: u32) {
    let mut store = STORE.lock();
    store.actors.remove(&boid);
    store.pending.remove(&boid);
}

/// Drop everything: match teardown and uninstall.
pub fn reset_all() {
    let mut store = STORE.lock();
    store.actors.clear();
    store.pending.clear();
}

/// Objects currently holding state, for diagnostics and tests.
pub fn boid_count() -> usize {
    STORE
        .lock()
        .actors
        .values()
        .filter(|actor| !actor.vars.is_empty())
        .count()
}

/// Variables held for one object, for diagnostics and tests.
pub fn var_count(boid: u32) -> usize {
    STORE
        .lock()
        .actors
        .get(&boid)
        .map(|actor| actor.vars.len())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated(name: &str) -> u32 {
        // Test boids live far above real object ids so parallel suites and a
        // live game can never share an entry.
        let boid = 0xF0000000
            | (name
                .bytes()
                .fold(0u32, |id, byte| id.wrapping_add(byte as u32))
                & 0x00FF_FFFF);
        reset_boid(boid);
        boid
    }

    #[test]
    fn writes_read_back_typed_and_unset_reads_none() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        let boid = isolated("typed");
        assert_eq!(get(boid, "gauge"), None);
        set(boid, "gauge", VarValue::Float(0.5));
        set(boid, "stocks", VarValue::Int(2));
        set(boid, "powered", VarValue::Flag(true));
        assert_eq!(get(boid, "gauge"), Some(VarValue::Float(0.5)));
        assert_eq!(get(boid, "stocks"), Some(VarValue::Int(2)));
        assert_eq!(get(boid, "powered"), Some(VarValue::Flag(true)));
        // Same name, different type overwrites: the store holds values, and
        // type discipline belongs to the validated IR, not the floor.
        set(boid, "gauge", VarValue::Int(1));
        assert_eq!(get(boid, "gauge"), Some(VarValue::Int(1)));
        reset_boid(boid);
    }

    #[test]
    fn teardown_clears_one_object_but_keeps_the_rest() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        let gone = isolated("gone");
        let kept = isolated("kept");
        set(gone, "gauge", VarValue::Float(1.0));
        set(kept, "gauge", VarValue::Float(2.0));
        reset_boid(gone);
        assert_eq!(get(gone, "gauge"), None);
        assert_eq!(var_count(gone), 0);
        assert_eq!(get(kept, "gauge"), Some(VarValue::Float(2.0)));
        reset_boid(kept);
    }

    #[test]
    fn reset_all_empties_the_store() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        let first = isolated("first");
        let second = isolated("second");
        set(first, "gauge", VarValue::Float(1.0));
        set(second, "gauge", VarValue::Float(1.0));
        reset_all();
        assert_eq!(boid_count(), 0);
        assert_eq!(get(first, "gauge"), None);
        assert_eq!(get(second, "gauge"), None);
    }

    #[test]
    fn reset_serializes_with_commit_and_never_resurrects_actor_state() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        let boid = isolated("reset-vs-commit");
        let identity = ActorIdentity {
            fighter_kind: 1,
            entry_id: 0,
            module_accessor_addr: 0x1234,
            stock_generation: 1,
        };
        let initial = begin(boid, identity);
        assert!(initial.commit(&BTreeMap::from([("gauge".to_string(), VarValue::Int(7),)])));
        let transaction = begin(boid, identity);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let reset_barrier = barrier.clone();
        let reset_thread = std::thread::spawn(move || {
            reset_barrier.wait();
            reset_boid(boid);
        });
        let commit_barrier = barrier.clone();
        let commit_thread = std::thread::spawn(move || {
            commit_barrier.wait();
            transaction.commit(&BTreeMap::from([("gauge".to_string(), VarValue::Int(99))]))
        });
        reset_thread.join().unwrap();
        let committed = commit_thread.join().unwrap();
        if committed {
            // Commit may win the mutex race, but reset must then remove its
            // result. If reset wins, commit must observe the invalidated row.
            assert_eq!(get(boid, "gauge"), None);
        }
        assert_eq!(get(boid, "gauge"), None);
    }

    #[test]
    fn exact_identity_recycle_starts_with_empty_authored_values() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        let boid = isolated("identity-recycle");
        let first = ActorIdentity {
            fighter_kind: 1,
            entry_id: 0,
            module_accessor_addr: 0x1234,
            stock_generation: 1,
        };
        let initial = begin(boid, first);
        assert!(initial.commit(&BTreeMap::from([("gauge".to_string(), VarValue::Int(7),)])));
        let recycled = ActorIdentity {
            module_accessor_addr: 0x5678,
            ..first
        };
        let next = begin(boid, recycled);
        assert_eq!(next.get("gauge"), None);
        assert!(next.commit(&BTreeMap::new()));
        assert_eq!(get(boid, "gauge"), None);
        reset_boid(boid);
    }

    #[test]
    fn direct_set_invalidates_pending_transaction_without_losing_the_direct_write() {
        let _serial = super::super::TEST_LOCK.lock().unwrap();
        let boid = isolated("direct-set-invalidates");
        let identity = ActorIdentity {
            fighter_kind: 1,
            entry_id: 0,
            module_accessor_addr: 0x1234,
            stock_generation: 1,
        };
        let transaction = begin(boid, identity);
        set(boid, "gauge", VarValue::Int(40));
        assert!(!transaction.commit(&BTreeMap::from([("gauge".to_string(), VarValue::Int(99),)])));
        assert_eq!(get(boid, "gauge"), Some(VarValue::Int(40)));
        reset_boid(boid);
    }
}
